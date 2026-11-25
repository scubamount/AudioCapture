"""Compare the v2.0.0 and current recorder windows on isolated Windows desktops."""

import argparse
from contextlib import ExitStack
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import time

import psutil

from test_live_performance import MIB, digest, sample, summarize, tone_peak, write_json
from test_live_ui_latency import (
    LB_GETCOUNT, LB_GETTEXT, LB_SETSEL, Native, PROCESS_LIST, SELECT_PROGRAMS,
    START, WM_CLOSE, WM_COMMAND, until,
)


def fixture_source(api, window, pid, baseline):
    if not baseline:
        api.button(window, SELECT_PROGRAMS)
    listing = api.control(window, PROCESS_LIST)
    until("visible source list", lambda: api.user.IsWindowVisible(listing))
    api.button(window, 108)  # Refresh

    def find_player():
        for index in range(api.send(listing, LB_GETCOUNT)):
            label = ctypes.create_unicode_buffer(32768)
            api.send(listing, LB_GETTEXT, index, ctypes.addressof(label))
            if f"PID {pid} " in label.value and "ffplay" in label.value.lower():
                return index + 1  # Index zero is valid.
        return None

    chosen = until("generated ffplay process", find_player)
    api.send(listing, LB_SETSEL, 0, -1)
    assert api.send(listing, LB_SETSEL, 1, chosen - 1) == 0
    api.send(window, WM_COMMAND, PROCESS_LIST | (1 << 16), listing)
    if not baseline:
        api.button(window, SELECT_PROGRAMS)


def sample_phase(subject, seconds):
    started = time.monotonic()
    readings = []
    while True:
        elapsed = time.monotonic() - started
        readings.append(sample(subject, elapsed))
        if elapsed >= seconds:
            break
        time.sleep(0.2)
    result = summarize(readings)
    result["cpu_seconds"] = readings[-1]["cpu_s"] - readings[0]["cpu_s"]
    result["max_sampled_working_set_bytes"] = max(row["working_set"] for row in readings)
    return result


def validate_tracks(recorded, count):
    tracks = sorted(recorded.glob("*.opus"))
    if len(tracks) != count:
        raise RuntimeError(f"Expected {count} Opus tracks, found {len(tracks)}")
    amplitudes = []
    for path in tracks:
        raw = subprocess.check_output([
            "ffmpeg", "-v", "error", "-i", path, "-t", "2", "-ar", "16000", "-ac", "1",
            "-f", "s16le", "-",
        ])
        signal = tone_peak(raw, 600)
        if signal < 4 or signal < 2 * tone_peak(raw, 750):
            raise RuntimeError("The recorded track does not contain the assigned 600 Hz source")
        amplitudes.append(round(signal, 3))
    return amplitudes


def run(api, executable, ffplay, folder, baseline, seconds, warm_restart):
    folder.mkdir()
    local, home, temp, recorded = [folder / n for n in ("local", "home", "temp", "recordings")]
    for directory in [local, home, temp, recorded]:
        directory.mkdir()
    env = dict(os.environ, LOCALAPPDATA=str(local), USERPROFILE=str(home), TMP=str(temp), TEMP=str(temp))
    prefs = local / "AudioCaptureRust" / "settings.json"
    prefs.parent.mkdir()
    prefs.write_text(json.dumps({"version": 1, "settings": {
        "folder": str(recorded), "format": "opus", "opus_kbps": 32, "channels": 1,
        "separate": True, "mixed": False, "skip_silence": False, "active_only": True,
        "system": False, "mic": None,
    }}), encoding="utf-8")
    with ExitStack() as stack:
        name, desktop = stack.enter_context(api.desktop())
        player = stack.enter_context(api.process(ffplay, name, env, folder, args=[
            "-nostats", "-loglevel", "quiet", "-volume", "100", "-window_title", "Recorder fixture",
            "-showmode", "1", "-f", "lavfi", "-i", "sine=frequency=600:sample_rate=48000,volume=0.0078125",
        ]))
        time.sleep(2)  # Allow WASAPI to create the renderer's audio session.
        recorder = stack.enter_context(api.process(executable, name, env, folder))
        window = until("recorder window", lambda: api.window(desktop, recorder.pid, "Audio Capture"))
        api.checked(api.user.SetWindowPos(window, None, 14, 14, 900, 700, 0x14))
        fixture_source(api, window, player.pid, baseline)
        api.button(window, START)
        until("first recording", lambda: list(recorded.glob("*.opus")))
        subject = psutil.Process(recorder.pid)
        phases = {key: sample_phase(subject, seconds) for key in ["cold", "steady"]}
        stop = 136 if baseline else START
        api.button(window, stop)
        time.sleep(1)
        phases["stopped"] = sample_phase(subject, seconds)
        if warm_restart:
            api.button(window, START)
            until("second recording", lambda: len(list(recorded.glob("*.opus"))) == 2)
            phases["warm_restart"] = sample_phase(subject, seconds)
            api.button(window, stop)
            time.sleep(0.6)
        api.post(window, WM_CLOSE)
        if api.kernel.WaitForSingleObject(recorder.process, 8000) != 0:
            raise RuntimeError("Recorder did not close after the final stop")
    signal = validate_tracks(recorded, 2 if warm_restart else 1)
    recorded_phases = [phases["cold"], phases["steady"]]
    cpu = 100 * sum(p["cpu_seconds"] for p in recorded_phases) / sum(p["elapsed_s"] for p in recorded_phases)
    return {"phases": phases, "overall_cpu_percent_one_core": cpu, "tone_amplitudes": signal}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True, help="v2.0.0 audio-capture.exe")
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="New private results directory")
    parser.add_argument("--pairs", type=int, default=5)
    parser.add_argument("--phase-seconds", type=int, default=10)
    args = parser.parse_args()
    if not 1 <= args.pairs <= 100 or not 2 <= args.phase_seconds <= 600:
        parser.error("Use 1–100 pairs and 2–600 seconds per phase")
    executables = {"baseline": args.baseline.resolve(strict=True), "candidate": args.candidate.resolve(strict=True)}
    ffplay = shutil.which("ffplay")
    if not ffplay or not shutil.which("ffmpeg"):
        parser.error("FFmpeg and ffplay must be on PATH")
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    api = Native()
    api.bind(api.user, "SetWindowPos", wintypes.BOOL, wintypes.HWND, wintypes.HWND, ctypes.c_int,
             ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT)
    result = {"sha256": {k: digest(v) for k, v in executables.items()}, "pairs": []}
    cpu_changes, peak_changes = [], []
    for pair in range(args.pairs):
        rows = {}
        for label in (["baseline", "candidate"] if pair % 2 == 0 else ["candidate", "baseline"]):
            print(f"Pair {pair + 1}: {label}", flush=True)
            rows[label] = run(api, executables[label], ffplay, root / f"{pair + 1}-{label}", label == "baseline",
                              args.phase_seconds, pair == 0)
        cpu_changes.append(rows["candidate"]["overall_cpu_percent_one_core"] - rows["baseline"]["overall_cpu_percent_one_core"])
        peak_changes.append(rows["candidate"]["phases"]["steady"]["peak_working_set_bytes"]
                            - rows["baseline"]["phases"]["steady"]["peak_working_set_bytes"])
        result["pairs"].append(rows)
        result["median_paired_cpu_delta_points"] = statistics.median(cpu_changes)
        result["max_paired_windows_peak_delta_bytes"] = max(peak_changes)
        write_json(root / "result.json", result)
    if args.pairs >= 5 and (statistics.median(cpu_changes) > 1 or max(peak_changes) > 2 * MIB):
        raise SystemExit("Native recording exceeded its CPU or lifetime peak working-set limit")


if __name__ == "__main__":
    main()
