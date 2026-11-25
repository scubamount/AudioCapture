"""Measure native Windows command acknowledgement during a stalled live request."""

import argparse
from array import array
from contextlib import ExitStack, contextmanager
import ctypes as c
from ctypes import wintypes as w
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import statistics
import subprocess
import sys
import tempfile
from threading import Event
from time import get_clock_info, monotonic, perf_counter_ns, sleep
import uuid
import wave

from test_live_worker import Harness, MockASR, wav_bytes


START, PAUSE, STOP_BOTH, LIVE = 134, 135, 146, 145
LIVE_STATUS, ENDPOINT, MODEL, LIVE_OUTPUT = 147, 149, 150, 158
PROCESS_LIST, SELECT_PROGRAMS, SOURCE_SUMMARY = 107, 171, 166
WM_COMMAND, WM_CLOSE = 0x111, 0x10
WM_GETTEXT, WM_SETTEXT = 0xD, 0xC
LB_GETCOUNT, LB_GETTEXT, LB_SETSEL = 0x18B, 0x189, 0x185
POLLS = 24


class Startup(c.Structure):
    _fields_ = [
        ("cb", w.DWORD), ("reserved", w.LPWSTR), ("desktop", w.LPWSTR), ("title", w.LPWSTR),
        ("x", w.DWORD), ("y", w.DWORD), ("cx", w.DWORD), ("cy", w.DWORD),
        ("xchars", w.DWORD), ("ychars", w.DWORD), ("fill", w.DWORD), ("flags", w.DWORD),
        ("show", w.WORD), ("reserved_size", w.WORD), ("reserved_data", c.c_void_p),
        ("stdin", w.HANDLE), ("stdout", w.HANDLE), ("stderr", w.HANDLE),
    ]


class ProcessInfo(c.Structure):
    _fields_ = [
        ("process", w.HANDLE), ("thread", w.HANDLE), ("pid", w.DWORD), ("tid", w.DWORD),
    ]


class JobLimit(c.Structure):
    _fields_ = [
        ("process_time", c.c_int64), ("job_time", c.c_int64), ("flags", w.DWORD),
        ("min_working_set", c.c_size_t), ("max_working_set", c.c_size_t),
        ("process_count", w.DWORD), ("affinity", c.c_size_t), ("priority", w.DWORD),
        ("scheduling", w.DWORD), ("io", c.c_uint64 * 6),
        ("process_memory", c.c_size_t), ("job_memory", c.c_size_t),
        ("peak_process_memory", c.c_size_t), ("peak_job_memory", c.c_size_t),
    ]


class Native:
    def __init__(self):
        self.user = c.WinDLL("user32", use_last_error=True)
        self.kernel = c.WinDLL("kernel32", use_last_error=True)
        self.callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
        bind = self.bind
        bind(self.user, "CreateDesktopW", w.HANDLE, w.LPCWSTR, w.LPCWSTR, c.c_void_p,
             w.DWORD, w.DWORD, c.c_void_p)
        bind(self.user, "GetThreadDesktop", w.HANDLE, w.DWORD)
        bind(self.user, "SetThreadDesktop", w.BOOL, w.HANDLE)
        bind(self.user, "CloseDesktop", w.BOOL, w.HANDLE)
        bind(self.user, "EnumDesktopWindows", w.BOOL, w.HANDLE, self.callback, w.LPARAM)
        bind(self.user, "GetWindowThreadProcessId", w.DWORD, w.HWND, c.POINTER(w.DWORD))
        bind(self.user, "GetWindowTextW", c.c_int, w.HWND, w.LPWSTR, c.c_int)
        bind(self.user, "GetDlgItem", w.HWND, w.HWND, c.c_int)
        bind(self.user, "IsWindowVisible", w.BOOL, w.HWND)
        bind(self.user, "IsWindowEnabled", w.BOOL, w.HWND)
        bind(self.user, "PostMessageW", w.BOOL, w.HWND, w.UINT, w.WPARAM, w.LPARAM)
        bind(self.user, "SendMessageTimeoutW", w.LPARAM, w.HWND, w.UINT, w.WPARAM,
             w.LPARAM, w.UINT, w.UINT, c.POINTER(c.c_size_t))
        bind(self.kernel, "GetCurrentThreadId", w.DWORD)
        bind(self.kernel, "CreateProcessW", w.BOOL, w.LPCWSTR, w.LPWSTR, c.c_void_p,
             c.c_void_p, w.BOOL, w.DWORD, c.c_void_p, w.LPCWSTR,
             c.POINTER(Startup), c.POINTER(ProcessInfo))
        bind(self.kernel, "CreateJobObjectW", w.HANDLE, c.c_void_p, w.LPCWSTR)
        bind(self.kernel, "AssignProcessToJobObject", w.BOOL, w.HANDLE, w.HANDLE)
        bind(self.kernel, "SetInformationJobObject", w.BOOL, w.HANDLE, c.c_int,
             c.c_void_p, w.DWORD)
        bind(self.kernel, "ResumeThread", w.DWORD, w.HANDLE)
        bind(self.kernel, "TerminateProcess", w.BOOL, w.HANDLE, w.UINT)
        bind(self.kernel, "WaitForSingleObject", w.DWORD, w.HANDLE, w.DWORD)
        bind(self.kernel, "GetExitCodeProcess", w.BOOL, w.HANDLE, c.POINTER(w.DWORD))
        bind(self.kernel, "CloseHandle", w.BOOL, w.HANDLE)

    @staticmethod
    def bind(dll, name, result, *params):
        function = getattr(dll, name)
        function.argtypes = params
        function.restype = result

    @staticmethod
    def checked(value):
        if not value:
            raise c.WinError(c.get_last_error())
        return value

    @contextmanager
    def desktop(self):
        name = f"AudioCaptureLatency-{uuid.uuid4().hex}"
        original = self.checked(self.user.GetThreadDesktop(self.kernel.GetCurrentThreadId()))
        desk = self.checked(self.user.CreateDesktopW(name, None, None, 0, 0x01FF, None))
        try:
            self.checked(self.user.SetThreadDesktop(desk))
            yield name, desk
        finally:
            self.user.SetThreadDesktop(original)
            self.user.CloseDesktop(desk)

    @contextmanager
    def process(self, executable, desktop, profile, cwd, args=()):
        job = self.checked(self.kernel.CreateJobObjectW(None, None))
        info = ProcessInfo()
        try:
            limit = JobLimit(flags=0x2000)  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            self.checked(self.kernel.SetInformationJobObject(job, 9, c.byref(limit), c.sizeof(limit)))
            startup = Startup(cb=c.sizeof(Startup), desktop=desktop)
            command = c.create_unicode_buffer(subprocess.list2cmdline(
                [str(executable), *map(str, args)]
            ))
            env = c.create_unicode_buffer("\0".join(
                f"{key}={value}" for key, value in sorted(profile.items(), key=lambda item: item[0].casefold())
            ) + "\0")
            # Attach the recorder and every process it launches before it runs.
            self.checked(self.kernel.CreateProcessW(
                str(executable), command, None, None, False, 0x404, env,
                str(cwd), c.byref(startup), c.byref(info),
            ))
            if not self.kernel.AssignProcessToJobObject(job, info.process):
                error = c.WinError(c.get_last_error())
                self.kernel.TerminateProcess(info.process, 1)
                raise error
            if self.kernel.ResumeThread(info.thread) == 0xFFFFFFFF:
                raise c.WinError(c.get_last_error())
            yield info
        finally:
            self.kernel.CloseHandle(job)
            for handle in (info.thread, info.process):
                if handle:
                    self.kernel.CloseHandle(handle)

    def window(self, desk, pid, title):
        matches = []

        @self.callback
        def inspect(hwnd, unused):
            owner = w.DWORD()
            self.user.GetWindowThreadProcessId(hwnd, c.byref(owner))
            if owner.value == pid and self.user.IsWindowVisible(hwnd):
                text = c.create_unicode_buffer(256)
                self.user.GetWindowTextW(hwnd, text, len(text))
                if text.value == title:
                    matches.append(hwnd)
            return True

        self.user.EnumDesktopWindows(desk, inspect, 0)
        return matches[0] if matches else None

    def send(self, window, message, first=0, second=0):
        result = c.c_size_t()
        self.checked(self.user.SendMessageTimeoutW(
            window, message, first, second, 2, 1000, c.byref(result),
        ))
        return c.c_ssize_t(result.value).value

    def post(self, window, message, first=0, second=0):
        self.checked(self.user.PostMessageW(window, message, first, second))

    def control(self, window, ident):
        return self.checked(self.user.GetDlgItem(window, ident))

    def read_text(self, control):
        text = c.create_unicode_buffer(8192)
        self.send(control, WM_GETTEXT, len(text), c.addressof(text))
        return text.value

    def set_text(self, control, value):
        text = c.create_unicode_buffer(str(value))
        self.send(control, WM_SETTEXT, 0, c.addressof(text))

    def button(self, window, ident):
        button = self.control(window, ident)
        assert self.user.IsWindowVisible(button) and self.user.IsWindowEnabled(button), ident
        self.post(window, WM_COMMAND, ident, button)


def until(label, test, seconds=15, poll=0.02):
    deadline = monotonic() + seconds
    while monotonic() < deadline:
        result = test()
        if result:
            return result
        sleep(poll)
    raise TimeoutError(f"Timed out waiting for {label}")


def make_tone(path, frequency):
    sample_rate = 48000
    frames = array("h", (
        int(4500 * math.sin(math.tau * frequency * i / sample_rate))
        for i in range(sample_rate)
    ))
    with wave.open(str(path), "wb") as audio:
        audio.setparams((1, 2, sample_rate, 0, "NONE", "not compressed"))
        audio.writeframes(frames.tobytes() * 45)


@contextmanager
def play_tone(path):
    make_tone(path, 700 if path.stem.endswith("one") else 1300)
    player = subprocess.Popen([sys.executable, "-B", __file__, "--play-tone", path],
                              creationflags=subprocess.CREATE_NO_WINDOW)
    try:
        yield player
    finally:
        if player.poll() is None:
            player.kill()
        player.wait(timeout=5)


def select_sources(api, app, pids):
    api.button(app, SELECT_PROGRAMS)
    listing = api.control(app, PROCESS_LIST)
    until("program picker", lambda: api.user.IsWindowVisible(listing))

    def active_sources():
        active = {}
        for index in range(api.send(listing, LB_GETCOUNT)):
            text = c.create_unicode_buffer(32768)
            api.send(listing, LB_GETTEXT, index, c.addressof(text))
            match = re.search(r"\bPID (\d+)\b", text.value)
            if match and int(match[1]) in pids:
                active[int(match[1])] = index
        return active if len(active) == len(pids) else None

    active = until("generated playback processes in the list", active_sources)
    api.send(listing, LB_SETSEL, 0, -1)
    for index in active.values():
        assert api.send(listing, LB_SETSEL, 1, index) == 0
    api.post(app, WM_COMMAND, PROCESS_LIST | (1 << 16), listing)  # LBN_SELCHANGE
    summary = api.control(app, SOURCE_SUMMARY)

    def selected_scope():
        text = api.read_text(summary)
        return text if all(f"PID {pid}" in text for pid in pids) else None

    result = until("the selected test PIDs", selected_scope)
    assert result.count("PID ") == len(pids), result
    api.button(app, SELECT_PROGRAMS)


def p95(samples):
    return sorted(samples)[math.ceil(len(samples) * 0.95) - 1]


def admission_snapshot(profile):
    root = profile / "AudioCaptureRust" / "batch-queue" / "admission"
    slots = {}
    for path in root.glob("request_*/request.json"):
        try:
            request = json.loads(path.read_text(encoding="utf-8"))
        except FileNotFoundError:
            continue
        slots[request["kind"]] = (path.parent / "granted").is_file()
    return slots


def acknowledge_pause(api, app, stall, batch, endpoint):
    assert api.read_text(api.control(app, LIVE)) == "Stop transcription"
    assert api.read_text(api.control(app, START)) == "Stop recording"
    pause = api.control(app, PAUSE)
    assert api.user.IsWindowVisible(pause) and api.user.IsWindowEnabled(pause)
    origin = perf_counter_ns()
    samples, timestamps, reads, polls = [], [], [], []
    for index in range(POLLS):
        assert stall.is_set() and batch.poll() is None
        assert [request.model for request in endpoint.requests] == ["live-ack-fixture"]
        paused = index % 2 == 0
        assert api.read_text(pause) == ("Pause recording" if paused else "Resume recording")
        expected = "Resume recording" if paused else "Pause recording"
        since = perf_counter_ns()
        api.post(app, WM_COMMAND, PAUSE, pause)
        last_poll = since
        while True:
            read_start = perf_counter_ns()
            polls.append((read_start - last_poll) / 1e6)
            state = api.read_text(pause)
            observed = perf_counter_ns()
            reads.append((observed - read_start) / 1e6)
            if state == expected:
                delay = (observed - since) / 1e6
                samples.append(delay)
                timestamps.append({
                    "command": "pause" if paused else "resume",
                    "input_ns": since - origin,
                    "observed_ns": observed - origin,
                    "elapsed_ms": delay,
                })
                break
            if observed - since > 1_000_000_000:
                raise TimeoutError(f"Expected {expected!r} after input")
            last_poll = read_start
            sleep(0.001)
        sleep(0.05)
    assert [request.model for request in endpoint.requests] == ["live-ack-fixture"]
    assert batch.poll() is None
    return {
        "samples_ms": samples,
        "acknowledgements": timestamps,
        "median_ms": statistics.median(samples),
        "p95_ms": p95(samples),
        "maximum_ms": max(samples),
        "observer_gettext_p95_ms": p95(reads),
        "observer_poll_start_gap_p95_ms": p95(polls),
        "observer_poll_start_gap_max_ms": max(polls),
        "perf_counter_resolution_seconds": get_clock_info("perf_counter").resolution,
    }


def run(recorder, root):
    worker = recorder.with_name("transcribe.exe")
    assert worker.is_file(), "Keep the recorder and worker together"
    harness = Harness(worker, root)
    harness.env.pop("AUDIOCAPTURE_GUI_PID", None)
    recordings, transcript, batch_output = [root / name for name in (
        "Recordings", "Live transcripts", "Batch output",
    )]
    for folder in (recordings, transcript, batch_output):
        folder.mkdir()
    item = root / "Completed recording.wav"
    item.write_bytes(wav_bytes(1000, 4800))
    settings = harness.local / "AudioCaptureRust" / "settings.json"
    settings.parent.mkdir()
    settings.write_text(json.dumps({
        "version": 1, "settings": {
            "format": "wav", "channels": 1, "active_only": False,
            "system": False, "folder": str(recordings),
        },
    }), encoding="utf-8")
    held, release = Event(), Event()

    def respond(request):
        if request.model == "live-ack-fixture":
            held.set()
            if not release.wait(25):
                raise TimeoutError("Fixture did not release the stalled request")
        return {"text": "Generated acknowledgement fixture."}

    api = Native()
    with ExitStack() as resources:
        endpoint = resources.enter_context(MockASR(response=respond))
        resources.callback(release.set)
        name, desk = resources.enter_context(api.desktop())
        players = [
            resources.enter_context(play_tone(root / f"tone-{suffix}.wav"))
            for suffix in ("one", "two")
        ]
        pids = [player.pid for player in players]
        process = resources.enter_context(api.process(recorder, name, harness.env, root))
        app = until("recorder window", lambda: api.window(desk, process.pid, "Audio Capture"))
        select_sources(api, app, pids)
        api.set_text(api.control(app, ENDPOINT), endpoint.url)
        api.set_text(api.control(app, MODEL), "live-ack-fixture")
        api.set_text(api.control(app, LIVE_OUTPUT), transcript)
        api.button(app, START)
        until("recording", lambda: api.read_text(api.control(app, START)) == "Stop recording")
        api.button(app, LIVE)
        consent = until("the live consent dialog", lambda: api.window(
            desk, process.pid, "Confirm live transcription",
        ))
        api.button(consent, 6)
        until("live transcription", lambda: api.read_text(api.control(app, LIVE)) == "Stop transcription")
        assert held.wait(8), api.read_text(api.control(app, LIVE_STATUS))
        hold_start = perf_counter_ns()
        answers = f"{endpoint.url}\nbatch-ack-fixture\n\n{batch_output}\nn\nn\nn\ny\n"
        with harness.child("--guided", "--", item) as batch:
            batch.send(answers.encode("utf-8"))
            row = batch.text(lambda line: "Batch ID: " in line)
            ident = re.search(r"Batch ID: (batch_[0-9a-f]{32})", row)[1]
            status = harness.run("batch-queue", "status", ident)
            assert re.search(r"\b(active|pending)\b", status), status
            expected_admission = {"Live": True, "Batch": False}
            until("a batch waiting for the granted live slot", lambda: (
                admission_snapshot(harness.local) == expected_admission
            ))
            assert not list(batch_output.glob("*.txt"))
            assert all(player.poll() is None for player in players)
            # Exercise the command path after the request has already stalled for a second.
            sleep(max(0, 1 - (perf_counter_ns() - hold_start) / 1e9))
            outcome = acknowledge_pause(api, app, held, batch.proc, endpoint)
            assert admission_snapshot(harness.local) == expected_admission
            outcome["requests_during_measurement"] = len(endpoint.requests)
            outcome["stall_confirmed_ms"] = (perf_counter_ns() - hold_start) / 1e6
            outcome["batch_status_during_measurement"] = status.strip()
            outcome["scheduler_grants_during_measurement"] = expected_admission
            api.button(app, STOP_BOTH)
            release.set()
            batch.done()
            assert any(request.model == "batch-ack-fixture" for request in endpoint.requests)
            until("final transcript", lambda: list(transcript.glob("*.txt")), seconds=20)
            until("both recording tracks", lambda: len(list(recordings.glob("*.wav"))) == len(pids))
        api.post(app, WM_CLOSE)
        assert api.kernel.WaitForSingleObject(process.process, 8000) == 0, "Recorder did not close"
        exit_code = w.DWORD()
        api.checked(api.kernel.GetExitCodeProcess(process.process, c.byref(exit_code)))
        assert exit_code.value == 0
        for track in recordings.glob("*.wav"):
            with wave.open(str(track), "rb") as audio:
                assert audio.getnframes() > 0, track
        timelines = list(recordings.glob("*.timeline.jsonl"))
        assert len(timelines) == 1, timelines
        with timelines[0].open(encoding="utf-8") as timeline:
            ranges = [
                entry for line in timeline
                if (entry := json.loads(line)).get("kind") == "pause"
            ]
        assert ranges and all(entry["end"] > entry["start"] for entry in ranges)
        outcome["recording_pause_ranges"] = len(ranges)
    assert all(player.returncode is not None for player in players)
    outcome["recorder_sha256"] = hashlib.file_digest(recorder.open("rb"), "sha256").hexdigest()
    outcome["worker_sha256"] = hashlib.file_digest(worker.open("rb"), "sha256").hexdigest()
    outcome["native_input"] = "Asynchronous Win32 WM_COMMAND with BN_CLICKED and the pause button HWND"
    outcome["observer"] = "Cross-thread synchronous WM_GETTEXT with 1 ms requested polling sleep"
    outcome["desktop"] = "Private Win32 desktop object and isolated LOCALAPPDATA"
    outcome["selected_generated_sources"] = len(players)
    outcome["windows_version"] = platform.version()
    outcome["logical_processors"] = os.cpu_count()
    outcome["threshold_p95_ms"] = 100
    outcome["pass"] = outcome["p95_ms"] <= 100
    return outcome


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("recorder", nargs="?", type=Path, help="Extracted audio-capture.exe")
    parser.add_argument("--output", type=Path, help="Directory to keep the receipt and generated files")
    parser.add_argument("--play-tone", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("This benchmark requires Windows")
    if args.play_tone:
        play = c.WinDLL("winmm").PlaySoundW
        play.argtypes = (w.LPCWSTR, w.HANDLE, w.DWORD)
        play.restype = w.BOOL
        if not play(str(args.play_tone), None, 0x00020002):
            raise SystemExit("Windows could not play the generated WAV")
        return
    if not args.recorder:
        parser.error("Supply the path to audio-capture.exe")
    root = Path(tempfile.mkdtemp(prefix="audio-capture-ui-latency-", dir=args.output))
    print(f"Evidence folder {root}", flush=True)
    outcome = run(args.recorder.resolve(strict=True), root)
    report = json.dumps(outcome, indent=2)
    (root / "latency.json").write_text(report + "\n", encoding="utf-8")
    print(report)
    if not outcome["pass"]:
        raise SystemExit("UI acknowledgement p95 exceeded 100 ms")


if __name__ == "__main__":
    main()
