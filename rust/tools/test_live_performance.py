"""Measure release executables on Windows. Developer requirements: psutil and FFmpeg.

The capture driver uses two generated program sources. It never selects a microphone
or the system playback endpoint. Run "capture --help" for paired and endurance runs.
"""

import argparse
import hashlib
import io
import json
import math
import os
from pathlib import Path
import re
import statistics
import struct
import subprocess
import sys
import threading
import time
import wave
from contextlib import ExitStack
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from test_live_worker import encode_audio, encode_control


RATE = 48_000
MIB = 1024 * 1024


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def write_json(path, value):
    temporary = path.with_suffix(".pending")
    temporary.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def fixture(path, source):
    """Quiet, deterministic, eight-second cycle. The two sources overlap in part."""
    with wave.open(str(path), "wb") as wav:
        wav.setparams((1, 2, RATE, 0, "NONE", "not compressed"))
        # Integer square waves avoid platform differences in math libraries.
        period = (96, 64)[source]
        second = b"".join(
            struct.pack("<h", 32 if frame % period < period // 2 else -32)
            for frame in range(RATE)
        )
        silence = bytes(2 * RATE)
        for index in range(8):
            wav.writeframes(second if index in ((0, 1, 4), (1, 3, 4))[source] else silence)


def player(path):
    import winsound

    winsound.PlaySound(str(path), winsound.SND_FILENAME | winsound.SND_NODEFAULT
                       | winsound.SND_LOOP | winsound.SND_ASYNC)
    print("READY", flush=True)
    sys.stdin.buffer.read(1)
    winsound.PlaySound(None, 0)


def spawn(command, stack, env=None):
    process = subprocess.Popen(
        command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        creationflags=subprocess.CREATE_NO_WINDOW, env=env,
    )

    def finish():
        if process.stdin and not process.stdin.closed:
            process.stdin.close()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
        process.stdout.close()
        process.stderr.close()

    stack.callback(finish)
    return process


def wait_ready(process):
    line = process.stdout.readline().decode("utf-8", "replace").strip()
    if line != "READY":
        raise RuntimeError(f"Expected READY, received {line!r}: "
                           + process.stderr.read(16_384).decode("utf-8", "replace"))


def sample(subject, elapsed):
    with subject.oneshot():
        memory = subject.memory_info()
        cpu = subject.cpu_times()
        return {
            "seconds": round(elapsed, 4),
            "cpu_s": cpu.user + cpu.system,
            "working_set": memory.rss,
            "peak_working_set": memory.peak_wset,
            "peak_private_commit": memory.peak_pagefile,
            "private": memory.private,
        }


def nearest_rank(values, quantile):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(quantile * len(ordered)) - 1)]


def tone_peak(pcm, frequency):
    """Peak amplitude at one fixture frequency over complete 100 ms windows."""
    samples = [sample for sample, in struct.iter_unpack("<h", pcm)]
    count = 1600
    weights = [complex(math.cos(2 * math.pi * frequency * n / 16_000),
                       math.sin(2 * math.pi * frequency * n / 16_000)) for n in range(count)]
    return max((abs(sum(value * weight for value, weight in zip(samples[start:start + count], weights)))
                * 2 / count for start in range(0, len(samples) - count + 1, count)), default=0)


def summarize(samples):
    first, last = samples[0], samples[-1]
    elapsed = last["seconds"] - first["seconds"]
    result = {
        "elapsed_s": round(elapsed, 3),
        "samples": len(samples),
        "cpu_percent_one_core": round((last["cpu_s"] - first["cpu_s"]) * 100 / elapsed, 4),
        "peak_working_set_bytes": max(s["peak_working_set"] for s in samples),
        "peak_private_commit_bytes": max(s["peak_private_commit"] for s in samples),
        "p95_sampled_working_set_bytes": nearest_rank([s["working_set"] for s in samples], 0.95),
    }
    for minute in [10, 30, 120]:
        mark = minute * 60
        near = [s["private"] for s in samples if mark - 5 <= s["seconds"] <= mark + 5]
        if near and last["seconds"] >= mark - 1:
            result[f"private_bytes_at_{minute}min"] = statistics.median(near)
    if "private_bytes_at_120min" in result and "private_bytes_at_30min" in result:
        result["private_growth_30_to_120min_bytes"] = (
            result["private_bytes_at_120min"] - result["private_bytes_at_30min"]
        )
    return result


def capture_run(exe, directory, seconds, player_ids):
    import psutil

    directory.mkdir()
    recorded = directory / "recorded"
    recorded.mkdir()
    samples = []
    children = set()
    with ExitStack() as processes, (directory / "resources.jsonl").open("x", encoding="utf-8") as log:
        process = spawn([exe, recorded, str(seconds), *map(str, player_ids)], processes)
        wait_ready(process)
        subject = psutil.Process(process.pid)
        started = time.monotonic()
        while process.poll() is None:
            try:
                reading = sample(subject, time.monotonic() - started)
                samples.append(reading)
                log.write(json.dumps(reading) + "\n")
                log.flush()
                children.update(p.name() for p in subject.children(recursive=True))
            except psutil.NoSuchProcess:
                break
            time.sleep(0.2 if seconds < 100 else 1)
        status = process.stdout.read(8192).decode("utf-8", "replace").strip()
        if process.wait():
            raise RuntimeError(status + "\n" + process.stderr.read().decode("utf-8", "replace"))
    outputs = []
    for path in sorted(recorded.glob("*.opus")):
        duration = float(subprocess.check_output(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", path],
            text=True))
        # Check one complete eight-second cycle. A very short probe may start in a silent phase.
        pcm = subprocess.check_output(["ffmpeg", "-v", "error", "-i", path, "-t", "8.1",
                                       "-f", "s16le", "-ac", "1", "-ar", "16000", "-"])
        expected = 500 if path.name.startswith("program-") else 750
        other = 750 if expected == 500 else 500
        wanted = tone_peak(pcm, expected)
        unwanted = tone_peak(pcm, other)
        if seconds >= 9 and (wanted < 4 or wanted < 2 * unwanted):
            raise RuntimeError(f"Expected {expected} Hz on {path.name}: {wanted=} {unwanted=}")
        outputs.append({"bytes": path.stat().st_size, "duration_s": duration,
                        "expected_hz": expected, "tone_amplitude": wanted, "other_amplitude": unwanted,
                        "max_abs_first_cycle": max((abs(x[0]) for x in struct.iter_unpack("<h", pcm)),
                                                   default=0)})
    if len(outputs) != 2 or min(p["duration_s"] for p in outputs) < seconds - 0.5:
        raise RuntimeError(f"Missing or short captured tracks: {outputs}")
    if seconds >= 9 and any(p["max_abs_first_cycle"] < 4 for p in outputs):
        raise RuntimeError(f"Synthetic signals were not captured: {outputs}")
    report = {"exe_sha256": digest(exe), "samples": summarize(samples),
              "observed_child_images": sorted(children), "status": status, "tracks": outputs}
    write_json(directory / "result.json", report)
    return report


def capture(args):
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    executables = {k: v.resolve(strict=True) for k, v in
                   [("baseline", args.baseline), ("candidate", args.candidate)] if v}
    if not executables:
        raise ValueError("Supply a baseline, candidate, or both")
    reports = []
    with ExitStack() as players:
        ids = []
        wave_hashes = []
        for source in range(2):
            path = root / f"tone-{source}.wav"
            fixture(path, source)
            wave_hashes.append(digest(path))
            process = spawn([sys.executable, "-B", __file__, "player", path], players)
            wait_ready(process)
            ids.append(process.pid)
        for pair in range(args.pairs):
            order = list(executables)
            if pair % 2:
                order.reverse()
            for label in order:
                print(f"Starting pair {pair + 1}: {label}", flush=True)
                report = capture_run(executables[label], root / f"{pair + 1}-{label}",
                                     args.seconds, ids)
                reports.append({"pair": pair + 1, "label": label, **report})
                write_json(root / "report.json", {"fixture_sha256": wave_hashes, "runs": reports})
                print(json.dumps(report["samples"]), flush=True)
                if report["samples"].get("private_growth_30_to_120min_bytes", 0) > 8 * MIB:
                    raise RuntimeError("Recorder private memory growth exceeded the reference limit")
    if len(executables) == 2:
        comparison = paired_deltas(reports)
        write_json(root / "report.json", {"fixture_sha256": wave_hashes, "runs": reports, "comparison": comparison})
        print(json.dumps(comparison, indent=2))
        if args.pairs >= 5 and (comparison["median_cpu_delta_points"] > 1
                                 or comparison["max_peak_working_set_delta_bytes"] > 2 * MIB):
            raise RuntimeError("Recording-only CPU or peak working set exceeded the reference limits")


def paired_deltas(reports):
    pairs = {}
    for row in reports:
        pairs.setdefault(row["pair"], {})[row["label"]] = row["samples"]
    cpu = []
    memory = []
    for pair in pairs.values():
        baseline, candidate = pair["baseline"], pair["candidate"]
        cpu.append(candidate["cpu_percent_one_core"] - baseline["cpu_percent_one_core"])
        memory.append(candidate["peak_working_set_bytes"] - baseline["peak_working_set_bytes"])
    return {
        "median_cpu_delta_points": round(statistics.median(cpu), 4),
        "p95_cpu_delta_points": round(nearest_rank(cpu, 0.95), 4),
        "median_peak_working_set_delta_bytes": statistics.median(memory),
        "max_peak_working_set_delta_bytes": max(memory),
    }


WIRE_FRAMES = 4_800
SIGNATURE = (10393, -2713)
TOKEN = re.compile(rb"\bsegment-\d{7}\b")


def audio_frame(source, tick, template):
    # These four samples bind a mock result to the exact source and session time.
    # They are generated benchmark input and never originate in a real recording.
    tag = struct.pack("<hhHI", *SIGNATURE, source, tick)
    return encode_audio(source, tick * WIRE_FRAMES, tag + template[len(tag):])


def timed_request(body, source_count):
    offset = body.index(b"RIFF")
    length, = struct.unpack_from("<I", body, offset + 4)
    with wave.open(io.BytesIO(body[offset:offset + length + 8]), "rb") as wav:
        if (wav.getframerate(), wav.getnchannels(), wav.getsampwidth()) != (RATE, 1, 2):
            raise ValueError("Expected mono 48 kHz PCM16 in multipart body")
        frames = wav.getnframes()
        first, second, source, tick = struct.unpack("<hhHI", wav.readframes(5))
    if (first, second) != SIGNATURE or source >= source_count or frames == 0:
        raise ValueError("The live worker submitted an unexpected source or frame boundary")
    start = tick * WIRE_FRAMES
    return source, start, start + frames


class LocalASR(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, delay, source_count):
        super().__init__(("127.0.0.1", 0), Reply)
        self.delay = delay
        self.source_count = source_count
        self.requests = []
        self.errors = []
        self.lock = threading.Lock()


class Reply(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        server = self.server
        try:
            size = int(self.headers["Content-Length"])
            if self.path != "/test/v1/audio/transcriptions" or not 0 < size <= 2 * MIB:
                raise ValueError("Unexpected request destination or length")
            source, first, end = timed_request(self.rfile.read(size), server.source_count)
            with server.lock:
                token = f"segment-{len(server.requests):07d}"
                row = {"token": token, "source": source, "first": first, "end": end,
                       "received_at": time.monotonic(), "bytes": size}
                server.requests.append(row)
            time.sleep(server.delay)
            answer = json.dumps({"text": token}).encode("ascii")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(answer)))
            self.end_headers()
            self.wfile.write(answer)
            row["response_at"] = time.monotonic()
        except (OSError, ValueError, KeyError, struct.error, wave.Error) as error:
            server.errors.append(str(error))
            self.send_error(400)


def observe_partial(local, output, visible, stop):
    target = None
    offset = 0
    tail = b""
    while not stop.is_set():
        if target is None:
            target = next(local.rglob("*.partial.txt"), None)
        if target is not None:
            try:
                with target.open("rb") as stream:
                    stream.seek(offset)
                    added = stream.read(64 * 1024)
                    offset = stream.tell()
                if added:
                    data = tail + added
                    now = time.monotonic()
                    for match in TOKEN.finditer(data):
                        visible.setdefault(match[0].decode("ascii"), now)
                    tail = data[-20:]
            except FileNotFoundError:
                target = None
        # Publication may replace the cache before our polling interval expires.
        if output.is_dir():
            for final in output.glob("*.txt"):
                if final.name.endswith(".partial.txt"):
                    continue
                now = time.monotonic()
                with final.open("rb") as stream:
                    for line in stream:
                        for match in TOKEN.finditer(line):
                            visible.setdefault(match[0].decode("ascii"), now)
        stop.wait(0.05)


def covers_interval(ranges, cutoff):
    ordered = sorted(ranges)
    return bool(ordered) and ordered[0][0] == 0 and ordered[-1][1] == cutoff and all(
        left[1] == right[0] for left, right in zip(ordered, ordered[1:])
    )


def worker(args):
    import psutil

    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    local, temp, output = (root / v for v in ("local", "temp", "text"))
    for folder in (local, temp, output):
        folder.mkdir()
    env = dict(os.environ, LOCALAPPDATA=str(local), TMP=str(temp), TEMP=str(temp))
    sources = [f"source-{i:02}" for i in range(16)] if args.pressure else ["program-fixture", "microphone-fixture"]
    server = LocalASR(args.delay, len(sources))
    stop = threading.Event()
    ready = threading.Event()
    visible = {}
    samples = []
    exe = args.worker.resolve(strict=True)
    temp_peak = 0
    command = [exe, "live", "--confirmed", "--base-url", f"http://127.0.0.1:{server.server_port}/test/v1",
               "--model", "local-performance-mock-v1", "--output-dir", output]
    for name in sources:
        command.extend(["--source", name])
    with ExitStack() as cleanup:
        cleanup.callback(server.server_close)
        listener = threading.Thread(target=server.serve_forever, daemon=True)
        listener.start()
        cleanup.callback(server.shutdown)
        proc = spawn(command, cleanup, env)
        subject = psutil.Process(proc.pid)

        def read_status():
            with (root / "status.jsonl").open("xb") as log:
                for line in proc.stdout:
                    log.write(line)
                    log.flush()
                    try:
                        status = json.loads(line)
                        if status.get("state") == "ready":
                            ready.set()
                    except (UnicodeError, ValueError):
                        pass

        status_reader = threading.Thread(target=read_status, daemon=True)
        status_reader.start()
        deadline = time.monotonic() + 30
        while not ready.wait(0.1):
            if proc.poll() is not None:
                raise RuntimeError(proc.stderr.read(4096).decode("utf-8", "replace"))
            if time.monotonic() >= deadline:
                raise RuntimeError("Live worker did not signal readiness")
        monitor = threading.Thread(target=observe_partial, args=(local, output, visible, stop), daemon=True)
        monitor.start()
        cleanup.callback(monitor.join, 2)
        cleanup.callback(stop.set)
        session = f"perf-{time.time_ns():x}"
        proc.stdin.write(encode_control("Start", session=session, interval=session + "-live",
                                        first=0, sources=sources))
        proc.stdin.flush()
        templates = [b"".join(struct.pack("<h", 1800 if n % p < p // 2 else -1800)
                              for n in range(WIRE_FRAMES)) for p in [96, 64]]
        started = time.monotonic()
        with (root / "resources.jsonl").open("x", encoding="utf-8") as log:
            for tick in range(args.seconds * 10):
                next_time = started + (tick + 1) * 0.1 / args.speed
                time.sleep(max(next_time - time.monotonic(), 0))
                for source in range(len(sources)):
                    proc.stdin.write(audio_frame(source, tick, templates[source % len(templates)]))
                proc.stdin.flush()
                if tick % max(1, int(10 * args.speed)) == 0:
                    reading = sample(subject, time.monotonic() - started)
                    temp_bytes = 0
                    for path in temp.rglob("*"):
                        try:
                            if path.is_file():
                                temp_bytes += path.stat().st_size
                        except FileNotFoundError:
                            pass
                    temp_peak = max(temp_peak, temp_bytes)
                    samples.append(reading)
                    log.write(json.dumps(reading) + "\n")
                    log.flush()
        stopped = time.monotonic()
        proc.stdin.write(encode_control("End", end=args.seconds * RATE))
        proc.stdin.close()
        drain_peak_private = 0
        drain_peak_rss = 0
        while proc.poll() is None:
            if time.monotonic() - stopped > 90:
                raise RuntimeError("Worker exceeded the test finalization timeout")
            try:
                ending = subject.memory_info()
                drain_peak_private = max(drain_peak_private, ending.peak_pagefile)
                drain_peak_rss = max(drain_peak_rss, ending.peak_wset)
            except psutil.NoSuchProcess:
                break
            time.sleep(0.05)
        proc.wait(timeout=2)
        status_reader.join(2)
        if proc.returncode:
            raise RuntimeError(proc.stderr.read(4096).decode("utf-8", "replace"))
        # Observe the publication before calculating end-to-text latency.
        last_publication = time.monotonic()
        monitor.join(0.15)
    if server.errors:
        raise RuntimeError(f"Mock server errors: {server.errors[:3]}")
    expected = args.seconds * RATE * len(sources)
    received = sum(row["end"] - row["first"] for row in server.requests)
    lags = []
    publish_lags = []
    for row in server.requests:
        when = visible.get(row["token"])
        row["visible_at"] = when
        if when is None:
            continue
        # Start after eight seconds of input, while retaining missing segments in the denominator.
        if row["end"] / RATE >= 8:
            lags.append(when - (started + row["end"] / RATE / args.speed))
            publish_lags.append(when - row["response_at"])
    missing = sum(row["token"] not in visible for row in server.requests)
    ordered = {}
    for row in server.requests:
        ordered.setdefault(row["source"], []).append((row["first"], row["end"]))
    cutoff = args.seconds * RATE
    contiguous = all(covers_interval(ordered.get(i, []), cutoff) for i in range(len(sources)))
    journals = list(output.glob("*.segments.jsonl"))
    coverage = {label: [] for label in sources}
    gap_frames = 0
    if len(journals) == 1:
        with journals[0].open(encoding="utf-8") as stream:
            for line in stream:
                row = json.loads(line)
                if row["type"] in ("speech", "gap"):
                    first, end = row["start_frame"], row["end_frame"]
                    coverage[row["source"]].append((first, end))
                    if row["type"] == "gap":
                        gap_frames += end - first
    accounted = all(covers_interval(ranges, cutoff) for ranges in coverage.values())
    final_text = [path for path in output.glob("*.txt") if not path.name.endswith(".partial.txt")]
    published = Counter()
    for path in final_text:
        with path.open("rb") as stream:
            for line in stream:
                published.update(token.decode("ascii") for token in TOKEN.findall(line))
    expected_tokens = Counter(row["token"] for row in server.requests)
    final_matches = len(final_text) == 1 and published == expected_tokens
    status_rows = [json.loads(line) for line in (root / "status.jsonl").read_bytes().splitlines()]
    terminal = status_rows[-1].get("state") if status_rows else None
    result = {
        "exe_sha256": digest(exe), "audio_seconds": args.seconds, "clock_speed": args.speed,
        "sources": len(sources), "explicit_gap_frames": gap_frames, "journal_covers_interval": accounted,
        "max_sampled_temp_bytes": temp_peak,
        "mock_delay_s": args.delay, "request_count": len(server.requests),
        "received_frames": received, "expected_frames": expected, "continuous_coverage": contiguous,
        "unobserved_results": missing, "final_text_matches": final_matches, "terminal_state": terminal,
        "p95_end_to_visible_s": nearest_rank(lags, 0.95) if lags else None,
        "p95_reply_to_visible_s": nearest_rank(publish_lags, 0.95) if publish_lags else None,
        "maximum_endpoint_response_s": max(
            (row["response_at"] - row["received_at"] for row in server.requests), default=None),
        "maximum_reply_to_visible_s": max(
            (visible[row["token"]] - row["response_at"] for row in server.requests
             if row["token"] in visible), default=None),
        "stop_to_exit_s": last_publication - stopped, "worker_resources": summarize(samples),
        "pressure_and_drain_peak_private_bytes": max(drain_peak_private, samples[-1]["peak_private_commit"]),
        "pressure_and_drain_peak_working_set_bytes": max(drain_peak_rss, samples[-1]["peak_working_set"]),
        "latency_observation": "50 ms polling on readable partial text, or final text if publication races observation",
    }
    write_json(root / "requests.json", server.requests)
    write_json(root / "result.json", result)
    print(json.dumps(result, indent=2))
    if not accounted or missing or not final_matches:
        raise RuntimeError("Live output lost, duplicated, or failed to commit fixture coverage")
    if args.pressure:
        if not gap_frames or received + gap_frames != expected or terminal != "incomplete":
            raise RuntimeError("Pressure test requires explicit, correctly bounded overload gaps")
    elif received != expected or not contiguous or gap_frames or terminal != "complete":
        raise RuntimeError("Balanced worker lost continuous coverage")
    if not args.pressure and args.speed == 1.0 and args.delay < 1:
        if result["maximum_endpoint_response_s"] > 1:
            raise RuntimeError("A mock reply exceeded the one-second reference condition")
        if result["p95_end_to_visible_s"] > 8 or result["maximum_reply_to_visible_s"] > 1:
            raise RuntimeError("Balanced worker exceeded the reference text latency limits")
    if result["worker_resources"].get("private_growth_30_to_120min_bytes", 0) > 8 * MIB:
        raise RuntimeError("Worker private memory growth exceeded the reference limit")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    play = sub.add_parser("player", help=argparse.SUPPRESS)
    play.add_argument("path", type=Path)
    record = sub.add_parser("capture", help="Profile real WASAPI recording of two generated signals")
    record.add_argument("--baseline", type=Path)
    record.add_argument("--candidate", type=Path)
    record.add_argument("--pairs", type=int, default=5)
    record.add_argument("--seconds", type=int, default=20)
    record.add_argument("--output", type=Path, required=True, help="New private results directory")
    live = sub.add_parser("worker", help="Measure real live worker with a two-source local mock endpoint")
    live.add_argument("--worker", type=Path, required=True)
    live.add_argument("--output", type=Path, required=True, help="New private results directory")
    live.add_argument("--seconds", type=int, default=60)
    live.add_argument("--delay", type=float, default=0.45, help="Mock ASR response delay in seconds")
    live.add_argument("--speed", type=float, default=1.0, help="Source clock speed, 1 for wall-clock qualification")
    live.add_argument("--pressure", action="store_true", help="Use 16 sources and require explicit overload gaps")
    args = parser.parse_args()
    if args.command == "player":
        player(args.path)
    elif args.command == "capture":
        if not 1 <= args.seconds <= 86_400 or not 1 <= args.pairs <= 100:
            parser.error("Expected 1-86400 seconds and 1-100 pairs")
        capture(args)
    elif args.command == "worker":
        if not 10 <= args.seconds <= 86_400 or not 0 <= args.delay <= 60 or not 0.25 <= args.speed <= 100:
            parser.error("Expected 10-86400 seconds, 0-60 seconds delay, and 0.25-100x clock speed")
        worker(args)


if __name__ == "__main__":
    main()
