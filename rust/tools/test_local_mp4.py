"""Compare local MP4 transcription through the native UI, JSON CLI, and MCP."""

import argparse
from contextlib import ExitStack, contextmanager
import ctypes as c
from ctypes import wintypes as w
import hashlib
import json
import msvcrt
import os
from pathlib import Path
from queue import Empty, Queue
import re
import shutil
import subprocess
import sys
import tempfile
from threading import Thread
from time import monotonic, sleep
import uuid

from test_live_ui_latency import JobLimit, Native, ProcessInfo, Startup, WM_CLOSE, until


OPEN_LOCAL, INPUT, OUTPUT = 141, 191, 194
PREVIEW, APPROVE, STATUS, REVIEW = 196, 197, 199, 200
IMPORT, MODEL_DIR, NEXT_PAGE = 201, 203, 205
REVISION = "911407f4214e0e1d82085af863093ec0b66f9cd6"
VERSION = {"schema_version": 1}
TERMINAL = {"completed", "failed", "cancelled", "interrupted"}
CONSOLE_CLASSES = {"ConsoleWindowClass", "CASCADIA_HOSTING_WINDOW_CLASS"}
ALLOWED_PROGRAMS = {"audio-capture.exe", "transcribe.exe", "conhost.exe"}
INPUT_INDICATORS = {"UAC_InputIndicatorOverlayWnd", "UAC Input Indicator"}


class JobAccounting(c.Structure):
    _fields_ = [
        ("user_time", c.c_int64), ("kernel_time", c.c_int64),
        ("period_user_time", c.c_int64), ("period_kernel_time", c.c_int64),
        ("page_fault_count", w.DWORD), ("process_count", w.DWORD),
        ("active_process_count", w.DWORD), ("terminated_process_count", w.DWORD),
    ]


def signature(path):
    with path.open("rb") as data:
        return hashlib.file_digest(data, "sha256").hexdigest()


class IsolatedDesktop(Native):
    def __init__(self):
        super().__init__()
        bind = self.bind
        bind(self.user, "OpenInputDesktop", w.HANDLE, w.DWORD, w.BOOL, w.DWORD)
        bind(self.user, "GetUserObjectInformationW", w.BOOL, w.HANDLE, c.c_int,
             c.c_void_p, w.DWORD, c.POINTER(w.DWORD))
        bind(self.user, "GetClassNameW", c.c_int, w.HWND, w.LPWSTR, c.c_int)
        bind(self.kernel, "QueryInformationJobObject", w.BOOL, w.HANDLE, c.c_int,
             c.c_void_p, w.DWORD, c.POINTER(w.DWORD))
        bind(self.kernel, "TerminateJobObject", w.BOOL, w.HANDLE, w.UINT)
        bind(self.kernel, "OpenProcess", w.HANDLE, w.DWORD, w.BOOL, w.DWORD)
        bind(self.kernel, "QueryFullProcessImageNameW", w.BOOL, w.HANDLE, w.DWORD,
             w.LPWSTR, c.POINTER(w.DWORD))

    def input_desktop(self):
        desk = self.checked(self.user.OpenInputDesktop(0, False, 1))
        try:
            name = c.create_unicode_buffer(256)
            self.checked(self.user.GetUserObjectInformationW(desk, 2, name, c.sizeof(name), None))
            return name.value
        finally:
            self.user.CloseDesktop(desk)

    def visible_windows(self, desk):
        windows = []

        @self.callback
        def inspect(hwnd, _):
            if self.user.IsWindowVisible(hwnd):
                name = c.create_unicode_buffer(256)
                title = c.create_unicode_buffer(256)
                pid = w.DWORD()
                self.user.GetClassNameW(hwnd, name, len(name))
                self.user.GetWindowTextW(hwnd, title, len(title))
                self.user.GetWindowThreadProcessId(hwnd, c.byref(pid))
                windows.append((pid.value, name.value, title.value))
            return True

        self.user.EnumDesktopWindows(desk, inspect, 0)
        return windows

    @contextmanager
    def job(self, desk, desktop):
        handle = self.checked(self.kernel.CreateJobObjectW(None, None))
        try:
            limit = JobLimit(flags=0x2000)
            self.checked(self.kernel.SetInformationJobObject(handle, 9, c.byref(limit), c.sizeof(limit)))
            yield Job(self, desk, desktop, handle)
        finally:
            self.kernel.CloseHandle(handle)


class Child:
    """Exchange piped lines with a process while its windows remain observable."""

    def __init__(self, job, info, stdin, stdout, stderr):
        self.job, self.info = job, info
        self.stdin = stdin
        self.stdout = Queue()
        self.readers = [
            Thread(target=self.read, args=(stdout, self.stdout), daemon=True),
            Thread(target=self.read, args=(stderr, None), daemon=True),
        ]
        for reader in self.readers:
            reader.start()

    def read(self, stream, queue):
        with stream:
            for line in stream:
                if queue is not None:
                    queue.put(line)
        if queue is not None:
            queue.put(None)

    def send(self, value):
        self.stdin.write(json.dumps(value, ensure_ascii=False).encode("utf-8") + b"\n")
        self.stdin.flush()

    def receive(self, seconds=60):
        deadline = monotonic() + seconds
        while monotonic() < deadline:
            self.job.observe()
            try:
                line = self.stdout.get(timeout=0.05)
            except Empty:
                continue
            if line is None:
                raise AssertionError("The command closed stdout before its JSON response.")
            try:
                return json.loads(line)
            except (ValueError, UnicodeError):
                raise AssertionError("The command wrote a non-JSON line to stdout.") from None
        raise TimeoutError("Timed out waiting for the command response.")

    def finish(self, seconds=10):
        self.stdin.close()
        kernel = self.job.api.kernel
        deadline = monotonic() + seconds
        while monotonic() < deadline:
            self.job.observe()
            if kernel.WaitForSingleObject(self.info.process, 25) == 0:
                break
        else:
            raise TimeoutError("The command did not exit after stdin closed.")
        for reader in self.readers:
            reader.join(timeout=2)
        assert all(not reader.is_alive() for reader in self.readers), "Child pipes stayed open."
        assert self.stdout.get(timeout=2) is None, "Unexpected extra output on stdout."
        code = w.DWORD()
        self.job.api.checked(kernel.GetExitCodeProcess(self.info.process, c.byref(code)))
        return code.value


class Job:
    """Contain the complete test process tree on an inactive private desktop."""

    def __init__(self, api, desk, desktop, handle):
        self.api, self.desk, self.desktop, self.handle = api, desk, desktop, handle
        self.gui_pid = None
        self.process_names = set()
        self.process_ids = set()

    def observe(self):
        for pid, name, title in self.api.visible_windows(self.desk):
            assert name not in CONSOLE_CLASSES, "A console became visible on the test desktop."
            main_window = name == "AudioCaptureRustWindow" and title == "Audio Capture"
            input_indicator = name in INPUT_INDICATORS and not title
            assert pid == self.gui_pid and (main_window or input_indicator), (
                f"An unexpected top-level window ({name}) became visible on the test desktop."
            )
        # Read only jobs started by this test. Process IDs may disappear during the query.
        counts = (w.DWORD * 2)()
        storage = c.create_string_buffer(8 + 32 * c.sizeof(c.c_size_t))
        if not self.api.kernel.QueryInformationJobObject(self.handle, 3, storage, len(storage), None):
            raise c.WinError(c.get_last_error())
        c.memmove(counts, storage, c.sizeof(counts))
        pids = (c.c_size_t * counts[1]).from_buffer(storage, 8)
        for pid in pids:
            if pid in self.process_ids:
                continue
            process = self.api.kernel.OpenProcess(0x1000, False, pid)
            if not process:
                continue
            try:
                name = c.create_unicode_buffer(32768)
                size = w.DWORD(len(name))
                if self.api.kernel.QueryFullProcessImageNameW(process, 0, name, c.byref(size)):
                    executable = Path(name.value).name.casefold()
                    self.process_names.add(executable)
                    self.process_ids.add(pid)
                    assert executable in ALLOWED_PROGRAMS, (
                        f"The app started an unexpected executable ({executable})."
                    )
            finally:
                self.api.kernel.CloseHandle(process)

    @contextmanager
    def process(self, executable, profile, cwd, args=(), *, gui=False, no_window=False):
        api = self.api
        info = ProcessInfo()
        startup = Startup(cb=c.sizeof(Startup), desktop=self.desktop)
        descriptors = []
        with ExitStack() as resources:
            if not gui:
                for _ in range(3):
                    pipe = os.pipe()
                    for fd in pipe:
                        file = resources.enter_context(os.fdopen(fd, "rb" if fd == pipe[0] else "wb"))
                        descriptors.append(file)
                start_in, stdin, stdout, start_out, stderr, start_err = descriptors
                startup.flags = 0x0100
                handles = [msvcrt.get_osfhandle(file.fileno()) for file in (start_in, start_out, start_err)]
                startup.stdin, startup.stdout, startup.stderr = handles
                for handle in handles:
                    os.set_handle_inheritable(handle, True)
            command = c.create_unicode_buffer(subprocess.list2cmdline([str(executable), *map(str, args)]))
            env = c.create_unicode_buffer("\0".join(
                f"{key}={value}" for key, value in sorted(profile.items(), key=lambda item: item[0].casefold())
            ) + "\0")
            flags = 0x404 | (subprocess.CREATE_NO_WINDOW if no_window else 0)
            try:
                api.checked(api.kernel.CreateProcessW(
                    str(executable), command, None, None, not gui, flags,
                    env, str(cwd), c.byref(startup), c.byref(info),
                ))
                api.checked(api.kernel.AssignProcessToJobObject(self.handle, info.process))
                if gui:
                    self.gui_pid = info.pid
                else:
                    for fd in (start_in, start_out, start_err):
                        fd.close()
                self.observe()
                if api.kernel.ResumeThread(info.thread) == 0xFFFFFFFF:
                    raise c.WinError(c.get_last_error())
                yield info if gui else Child(self, info, stdin, stdout, stderr)
            except BaseException:
                # End contained subprocesses before closing their redirected pipes.
                api.kernel.TerminateJobObject(self.handle, 1)
                raise
            finally:
                if info.process:
                    if api.kernel.WaitForSingleObject(info.process, 0) != 0:
                        api.kernel.TerminateProcess(info.process, 1)
                    api.kernel.CloseHandle(info.process)
                if info.thread:
                    api.kernel.CloseHandle(info.thread)

    def metrics(self):
        info = JobLimit()
        self.api.checked(self.api.kernel.QueryInformationJobObject(
            self.handle, 9, c.byref(info), c.sizeof(info), None,
        ))
        accounting = JobAccounting()
        self.api.checked(self.api.kernel.QueryInformationJobObject(
            self.handle, 1, c.byref(accounting), c.sizeof(accounting), None,
        ))
        return {
            "observed_executables": sorted(self.process_names),
            "observed_process_count": len(self.process_ids),
            "job_process_count": accounting.process_count,
            "cpu_seconds": round((accounting.user_time + accounting.kernel_time) / 1e7, 2),
            "peak_process_memory_bytes": info.peak_process_memory,
            "peak_job_memory_bytes": info.peak_job_memory,
        }


class Cli:
    def __init__(self, job, worker, env, root):
        self.job, self.worker, self.env, self.root = job, worker, env, root

    def call(self, group, action, fields=None, exit_code=0):
        args = ["control", group, action, "--json", "--input", "-"]
        with self.job.process(self.worker, self.env, self.root, args, no_window=True) as child:
            child.send(VERSION | (fields or {}))
            child.stdin.close()
            result = child.receive()
            assert child.finish() == exit_code, f"Unexpected CLI exit for {group}.{action}."
            assert isinstance(result, dict) and result.get("schema_version") == 1
            assert result.get("ok") is (exit_code == 0), f"Unexpected CLI result for {group}.{action}."
            return result


class Mcp:
    def __init__(self, child):
        self.child = child
        self.serial = 0

    def request(self, method, params=None):
        self.serial += 1
        self.child.send({"jsonrpc": "2.0", "id": self.serial, "method": method, "params": params or {}})
        response = self.child.receive()
        assert response.get("jsonrpc") == "2.0" and response.get("id") == self.serial
        assert "result" in response, f"MCP returned an error for {method}."
        return response["result"]

    def initialize(self):
        result = self.request("initialize", {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "audio-capture-acceptance", "version": "1"},
        })
        assert result["protocolVersion"] == "2025-11-25"
        assert "tools" in result["capabilities"]
        self.child.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        assert self.request("ping") == {}
        tools = {tool["name"]: tool for tool in self.request("tools/list")["tools"]}
        assert {"audio_capture_batch", "audio_capture_transcript", "audio_capture_profile"} <= tools.keys()
        assert "audio_capture_grants" not in tools
        assert all(tool[schema]["type"] == "object" for tool in tools.values()
                   for schema in ("inputSchema", "outputSchema"))

    def call(self, group, action, fields=None, exit_code=0):
        result = self.request("tools/call", {
            "name": f"audio_capture_{group}", "arguments": VERSION | {"action": action} | (fields or {}),
        })
        assert bool(result.get("isError")) is (exit_code != 0), f"Unexpected MCP state for {group}.{action}."
        data = result["structuredContent"]
        assert data.get("ok") is (exit_code == 0) and data.get("schema_version") == 1
        return data


def create_profile(root):
    root.mkdir()
    profile = root / "appdata"
    temp = root / "temp"
    output = root / "transcripts"
    for folder in (profile, temp, output):
        folder.mkdir()
    env = {key: value for key, value in os.environ.items() if not key.upper().startswith((
        "AUDIOCAPTURE_", "AUDIO_CAPTURE_", "RAYON_", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY",
    ))}
    env.update(PATH="", LOCALAPPDATA=str(profile), APPDATA=str(profile), TEMP=str(temp), TMP=str(temp),
               AUDIO_CAPTURE_STATE_DIR=str(root / "state"))
    return output, env


def accepted(call, group, plan, key=None):
    return call(group, "commit", {
        "plan_id": plan["plan_id"], "approved": True, "idempotency_key": key or str(uuid.uuid4()),
    })


def install(call, model):
    request = {"operation": "import_model", "source_dir": str(model)}
    initial, = call("profile", "model_list")["models"]
    assert initial["installed"] is False
    accepted(call, "profile", call("profile", "preview", request))
    installed, = call("profile", "model_list")["models"]
    assert installed["installed"] is True and installed["revision"] == REVISION


def submit(call, video, output):
    request = {
        "operation": "transcribe", "profile_id": "local-whisper",
        "files": [str(video)], "output_dir": str(output),
    }
    plan = call("batch", "preview", request)
    scope = plan["scope"]
    assert scope["provider"] == "local" and scope["profile_id"] == "local-whisper"
    assert scope["model_revision"] == REVISION
    assert scope["audio_track"] == "first" and scope["timestamps"] is True
    assert Path(scope["files"][0]).samefile(video) and Path(scope["output_dir"]).samefile(output)
    key = str(uuid.uuid4())
    rejected = call("batch", "commit", {
        "plan_id": plan["plan_id"], "approved": False, "idempotency_key": key,
    }, exit_code=2)
    assert rejected["error"]["code"] == "invalid_input"
    assert not call("batch", "list")["batch_ids"], "Preview or rejection submitted work."
    receipt = accepted(call, "batch", plan, key)
    assert accepted(call, "batch", plan, key)["batch_id"] == receipt["batch_id"]
    assert receipt["batch_id"] in call("batch", "list")["batch_ids"]
    return receipt["batch_id"]


def transcript(call, batch_id, output, seconds, poll_interval=0.8):
    deadline = monotonic() + seconds
    request = {"batch_id": batch_id}
    seen = []
    while monotonic() < deadline:
        result = call("batch", "status", request)
        if result["state"] not in seen:
            seen.append(result["state"])
        if result["state"] in TERMINAL:
            break
        sleep(poll_interval)
    else:
        raise TimeoutError("Timed out waiting for local transcription.")
    assert result["state"] == "completed", f"Unexpected terminal state {result['state']}."
    frames = result["processed_frames"]
    assert frames > 0
    page = call("transcript", "fetch", request | {"offset": 0, "limit": 24})
    segments = list(page["segments"])
    offset = page.get("next_offset")
    while offset is not None:
        assert offset == len(segments), "Transcript paging skipped or repeated segments."
        page = call("transcript", "fetch", request | {"offset": offset, "limit": 24})
        segments.extend(page["segments"])
        offset = page.get("next_offset")
    assert len(segments) == result["segment_count"]
    text_path, json_path = Path(page["text_path"]), Path(page["json_path"])
    # File identity handles both ordinary and extended-length Windows path syntax.
    assert text_path.parent.parent.samefile(output)
    assert json_path.parent.parent.samefile(output)
    return result_files(text_path, json_path, expected=segments, frames=frames) | {"observed_states": seen}


def result_files(text_path, json_path, expected=None, frames=None):
    payload = json.loads(json_path.read_bytes())
    assert payload["provider"] == "local" and payload["model_revision"] == REVISION
    assert payload["partial"] is False and payload["sample_rate"] == 16_000
    assert frames is None or payload["source_frames"] == frames
    frames = payload["source_frames"]
    assert frames > 0
    segments = [{key: item[key] for key in ("start_ms", "end_ms", "text")} for item in payload["segments"]]
    if expected is not None:
        assert [{key: item[key] for key in ("start_ms", "end_ms", "text")} for item in expected] == segments
    last_end = 0
    for item in segments:
        start, end, text = item["start_ms"], item["end_ms"], item["text"]
        assert isinstance(start, int) and isinstance(end, int) and last_end <= start < end
        assert text.strip()
        last_end = end
    assert last_end <= frames * 1000 // 16_000 + 1, "A timestamp exceeded the decoded audio duration."
    text = text_path.read_text(encoding="utf-8")
    assert payload["text"] == text, "Text and structured artifacts differ."
    words = len(text.split())
    assert words > 0 and segments
    return {
        "words": words, "segments": len(segments), "first_start_ms": segments[0]["start_ms"],
        "last_end_ms": last_end, "decoded_frames": frames,
        "audio_seconds": round(frames / 16_000, 3),
        "input_sha256": payload["source_sha256"],
        "text_sha256": signature(text_path), "json_sha256": signature(json_path),
        "timed_text_sha256": hashlib.sha256(json.dumps(segments, sort_keys=True).encode()).hexdigest(),
        "text_path": str(text_path), "json_path": str(json_path),
    }


def run_cli_or_mcp(api, desktop, desk, recorder, model, video, root, seconds, *, mcp):
    output, env = create_profile(root)
    with api.job(desk, desktop) as job:
        cli = Cli(job, recorder.with_name("transcribe.exe"), env, root)
        capabilities = cli.call("diagnostics", "capabilities")
        assert capabilities["provider"] == "local" and "mp4" in capabilities["file_types"]
        denied = cli.call("batch", "list", exit_code=3)
        assert denied["error"]["code"] == "authorization_denied"
        grant = cli.call("grants", "provision", {"policy": {
            "input_roots": [str(video), str(model)], "output_roots": [str(output)],
            "operations": ["batch.transcribe", "batch.status", "batch.cancel", "transcript.fetch", "model.import"],
            "profiles": ["local-whisper"], "allow_headless_approval": True,
        }})
        env["AUDIO_CAPTURE_GRANT_REF"] = grant["grant_ref"]
        if mcp:
            # Start the GUI-subsystem launcher with ordinary creation flags and redirected pipes.
            with job.process(recorder, env, root, ("mcp",)) as child:
                client = Mcp(child)
                client.initialize()
                install(client.call, model)
                started = monotonic()
                ident = submit(client.call, video, output)
                assert child.finish() == 0
            with job.process(recorder, env, root, ("mcp",)) as child:
                client = Mcp(child)
                client.initialize()
                result = transcript(client.call, ident, output, seconds, poll_interval=0.25)
                assert child.finish() == 0
        else:
            install(cli.call, model)
            started = monotonic()
            ident = submit(cli.call, video, output)
            result = transcript(cli.call, ident, output, seconds)
        result.update(job.metrics(), elapsed_seconds=round(monotonic() - started, 2))
        assert "transcribe.exe" in job.process_names
    return result


def run_ui(api, desktop, desk, recorder, model, video, root, seconds):
    output, env = create_profile(root)
    with api.job(desk, desktop) as job, job.process(recorder, env, root, gui=True) as process:
        def observe(test):
            job.observe()
            return test()

        app = until("recorder window", lambda: observe(lambda: api.window(desk, process.pid, "Audio Capture")))
        api.button(app, OPEN_LOCAL)
        status_control = api.control(app, STATUS)
        import_button = api.control(app, IMPORT)
        until("native local file panel", lambda: observe(lambda: (
            api.read_text(status_control).startswith("Extract the English base.en model pack.")
            and api.user.IsWindowVisible(import_button)
            and api.user.IsWindowEnabled(import_button)
        )))
        api.set_text(api.control(app, MODEL_DIR), model)
        api.button(app, IMPORT)
        until("verified model import", lambda: observe(lambda: (
            api.read_text(status_control).startswith("Model installed.")
            and api.user.IsWindowEnabled(api.control(app, PREVIEW))
        )), seconds=180)
        api.set_text(api.control(app, INPUT), video)
        api.set_text(api.control(app, OUTPUT), output)
        api.button(app, PREVIEW)
        until("approval-ready preview", lambda: observe(lambda: (
            api.read_text(status_control).startswith("Ready to transcribe")
            and api.user.IsWindowEnabled(api.control(app, APPROVE))
        )))
        assert not list(output.rglob("*.txt")), "Preview started work before approval."
        started = monotonic()
        api.button(app, APPROVE)
        seen = set()

        def completed():
            job.observe()
            status = api.read_text(status_control)
            for state in ("queued", "running", "failed", "cancelled", "interrupted"):
                if status.startswith(f"Local job {state}."):
                    assert state not in TERMINAL, f"The native job entered {state}."
                    seen.add(state)
            return status.startswith("Completed.")

        until("local job completion", completed, seconds=seconds, poll=0.1)
        review = until("timed transcript in the native UI", lambda: observe(lambda: (
            text if re.search(r"\d{1,2}:\d{2}[:.]\d{2}", text := api.read_text(api.control(app, REVIEW))) else None
        )), seconds=12)
        json_path, = output.rglob("*.json")
        text_path, = output.rglob("*.txt")
        result = result_files(text_path, json_path)
        first = json.loads(json_path.read_bytes())["segments"][0]["text"].strip()
        assert first in review, "The native review omitted the first timed text."
        result.update(job.metrics(), elapsed_seconds=round(monotonic() - started, 2))
        if api.user.IsWindowEnabled(api.control(app, NEXT_PAGE)):
            api.button(app, NEXT_PAGE)
            until("the second timed page", lambda: observe(lambda: (
                (text := api.read_text(api.control(app, REVIEW))) != review
                and re.search(r"\d{1,2}:\d{2}[:.]\d{2}", text)
            )))
            result["native_review_pagination"] = True
        result["observed_running_states"] = sorted(seen)
        api.post(app, WM_CLOSE)
        assert api.kernel.WaitForSingleObject(process.process, 8000) == 0, "Recorder did not close."
    assert "transcribe.exe" in job.process_names, "The UI did not use the packaged Rust worker."
    return result


def check_old_cli(worker, root):
    """Reproduce the missing machine interface on the predecessor without opening a window."""
    _, env = create_profile(root)
    result = subprocess.run(
        [worker, "control", "diagnostics", "capabilities", "--json", "--input", "-"],
        input=json.dumps(VERSION).encode() + b"\n", capture_output=True, cwd=root, env=env,
        creationflags=subprocess.CREATE_NO_WINDOW, timeout=15,
    )
    assert result.returncode != 0, "The baseline unexpectedly supports local control."
    return {"exit": result.returncode, "has_machine_capabilities": False}


def run(args, root):
    api = IsolatedDesktop()
    before = api.input_desktop()
    package = root / "package"
    package.mkdir()
    recorder = Path(shutil.copy2(args.recorder, package / "audio-capture.exe"))
    worker = recorder.with_name("transcribe.exe")
    shutil.copy2(args.recorder.with_name("transcribe.exe"), worker)
    input_hash = signature(args.video)
    result = {"windows_version": sys.getwindowsversion().build, "input_sha256": input_hash,
              "model_revision": REVISION, "model_sha256": signature(args.model / "model.safetensors"),
              "paths": {}}
    if args.baseline_worker:
        result["baseline"] = check_old_cli(args.baseline_worker, root / "baseline")
    with api.desktop() as (desktop, desk):
        assert api.input_desktop() != desktop, "The test desktop must stay inactive."
        for surface in args.surfaces:
            print(f"Testing the {surface} path", flush=True)
            if surface == "ui":
                data = run_ui(api, desktop, desk, recorder, args.model, args.video, root / surface,
                              args.timeout)
            else:
                data = run_cli_or_mcp(api, desktop, desk, recorder, args.model, args.video,
                                      root / surface, args.timeout, mcp=surface == "mcp")
            assert data["input_sha256"] == input_hash
            data["realtime_factor"] = round(data["elapsed_seconds"] / data["audio_seconds"], 3)
            result["paths"][surface] = data
            assert api.input_desktop() != desktop
            # Retain per-surface evidence if a later surface fails.
            (root / "partial.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    assert before == api.input_desktop(), "The active desktop changed during the test."
    assert signature(args.video) == input_hash, "The supplied MP4 changed."
    assert not list(root.rglob("*.wav")), "The local workflow left a WAV file."
    paths = result["paths"].values()
    assert len({data["text_sha256"] for data in paths}) == 1, "Transcript text differs by entry point."
    assert len({data["timed_text_sha256"] for data in paths}) == 1, "Timed segments differ by entry point."
    assert len({data["json_sha256"] for data in paths}) == 1, "Structured output differs by entry point."
    result.update({
        "private_desktop": True, "input_desktop_unchanged": True, "console_windows_observed": 0,
        "empty_worker_path": True, "recorder_sha256": signature(recorder),
        "worker_sha256": signature(worker),
    })
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("recorder", type=Path, help="Path to audio-capture.exe next to transcribe.exe")
    parser.add_argument("video", type=Path, help="Local MP4 input")
    parser.add_argument("model", type=Path, help="Directory of extracted base.en model files")
    parser.add_argument("--output", type=Path, required=True, help="Private parent directory for the results")
    parser.add_argument("--baseline-worker", type=Path, help="Previous transcribe.exe for a capability probe")
    parser.add_argument("--surfaces", nargs="+", choices=("ui", "cli", "mcp"), default=("ui", "cli", "mcp"))
    parser.add_argument("--timeout", type=float, default=900, help="Maximum inference wait per surface")
    args = parser.parse_args()
    for field in ("recorder", "video", "model", "baseline_worker"):
        if value := getattr(args, field):
            setattr(args, field, value.resolve(strict=True))
    assert args.recorder.with_name("transcribe.exe").is_file()
    assert len(set(args.surfaces)) == len(args.surfaces)
    root = Path(tempfile.mkdtemp(prefix="native-mp4-", dir=args.output.resolve(strict=True)))
    print(f"Evidence folder {root}", flush=True)
    report = run(args, root)
    (root / "acceptance.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({surface: {key: data[key] for key in ("words", "segments", "elapsed_seconds")}
                      for surface, data in report["paths"].items()}, indent=2))


if __name__ == "__main__":
    main()
