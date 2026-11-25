"""Compare the packaged worker's speech gate using a local Whisper endpoint.

Requires Python with faster-whisper. Point --model to previously installed
weights. The HTTP server listens on an ephemeral loopback port. No input or
model is downloaded. Keep each new output directory private.
"""

import argparse
from collections import Counter
from email import policy
from email.parser import BytesParser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hashlib
import io
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import threading
import time
import uuid
import wave

import test_speaker_acceptance as offline


def words(text):
    return re.findall(r"[a-z0-9']+", text.lower())


def word_alignment(ref, heard):
    """Return S/D/I counts and the reference indices matched by an optimal path."""
    # With equal edit distance, favor more exact word matches before deciding
    # substitutions/deletions/insertions. Keep quiet-word attribution stable.
    prev = [(j, 0) for j in range(len(heard) + 1)]
    paths = [bytearray([2] * len(prev))]
    for i, word in enumerate(ref, 1):
        row = [(i, 0)]
        ops = bytearray([1])
        for j, candidate in enumerate(heard, 1):
            same = word == candidate
            scores = ((prev[j - 1][0] + (not same), prev[j - 1][1] - same),
                      (prev[j][0] + 1, prev[j][1]), (row[-1][0] + 1, row[-1][1]))
            op = min(range(3), key=scores.__getitem__)
            row.append(scores[op])
            ops.append(op)
        paths.append(ops)
        prev = row
    counts, matched = Counter(), set()
    i, j = len(ref), len(heard)
    while i or j:
        op = paths[i][j]
        if op == 0:
            i -= 1
            j -= 1
            if ref[i] == heard[j]:
                matched.add(i)
            else:
                counts["substitutions"] += 1
        elif op == 1:
            i -= 1
            counts["deletions"] += 1
        else:
            j -= 1
            counts["insertions"] += 1
    return counts, matched


def wire(kind, payload):
    return struct.pack("<4sBI", b"ACL1", kind, len(payload)) + payload


def control(label, data):
    return wire(0, json.dumps({label: data}, separators=(",", ":")).encode("utf-8"))


def samples48(path):
    return subprocess.check_output([
        "ffmpeg", "-nostdin", "-v", "error", "-i", str(path), "-ar", "48000",
        "-ac", "1", "-f", "s16le", "-",
    ])


def paced_worker(worker, base_url, output, sources, gated, speed, speaker_config=None, frame_ranges=None):
    interval = "quality-" + uuid.uuid4().hex
    output.mkdir(parents=True, exist_ok=False)
    profile = output / "local"
    profile.mkdir()
    command = [str(worker), "live", "--confirmed", "--base-url", base_url, "--model", "local",
               "--output-dir", str(output), "--language", "en"]
    for name in sources:
        command.extend(["--source", name])
    if gated:
        command.extend(["--speech-gate", "--evaluate-gate"])
    if speaker_config is not None:
        command.extend(["--speaker-config", str(speaker_config), "--evaluate-speakers", "--timestamps"])
    env = dict(os.environ, LOCALAPPDATA=str(profile))
    with (output / "worker.stderr").open("wb") as errors:
        proc = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=errors, env=env)
        try:
            ready = proc.stdout.readline(8192)
            if not ready or json.loads(ready).get("state") != "ready":
                raise RuntimeError("Live worker did not admit test process. See worker.stderr.")
            states = []

            def read_status():
                for line in proc.stdout:
                    state = json.loads(line)
                    states.append((state.get("state"), state.get("path")))

            reader = threading.Thread(target=read_status)
            reader.start()
            proc.stdin.write(control("Start", {"session": interval, "interval": interval,
                                               "first": 0, "sources": list(sources)}))
            if speaker_config is not None:
                proc.stdin.write(control("Speakers", {"enabled": True, "frame": 0}))
            frames = max(len(pcm) // 2 for pcm in sources.values())
            start = time.monotonic()
            for first in range(0, frames, 4800):
                for index, pcm in enumerate(sources.values()):
                    part = pcm[first * 2:(first + 4800) * 2]
                    if part:
                        proc.stdin.write(wire(1, struct.pack("<HQ", index, first) + part))
                proc.stdin.flush()
                due = start + min(frames, first + 4800) / (48000 * speed)
                time.sleep(max(0.0, due - time.monotonic()))
            proc.stdin.write(control("End", {"end": frames}))
            proc.stdin.close()
            if proc.wait(timeout=45):
                raise RuntimeError("Live worker failed. See worker.stderr.")
            reader.join(timeout=2)
            if reader.is_alive() or not any(s == "complete" for s, _ in states):
                raise RuntimeError(f"Live worker did not report complete. States: {[s for s, _ in states][-5:]}")
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
    indices = list(output.glob("*.segments.jsonl"))
    texts = list(output.glob("*.txt"))
    if len(indices) != 1 or len(texts) != 1:
        raise RuntimeError("Expected one finalized transcript and index")
    events = [json.loads(line) for line in indices[0].read_text(encoding="utf-8").splitlines()]
    if any(event["type"] == "gap" or event.get("incomplete") for event in events):
        raise RuntimeError("ASR comparison contains incomplete coverage. Reduce the replay speed.")
    recognized = {name: " ".join(e["text"] for e in events if e["type"] == "speech" and e["source"] == name)
                  for name in sources}
    if frame_ranges is not None:
        frame_ranges.extend((e["source"], e["start_frame"], e["end_frame"]) for e in events if e["type"] == "request")
    return recognized, texts[0].stat().st_size


class LocalRecognizer:
    def __init__(self, model_dir):
        from faster_whisper import WhisperModel
        self.model = WhisperModel(str(model_dir), device="cpu", compute_type="int8",
                                  cpu_threads=2, local_files_only=True)
        self.requests = []
        self.payloads = []
        self.service_seconds = []
        self.prepare = False
        self.inputs = {}
        self.answers = None

    def recognize(self, wav, transferred):
        with wave.open(io.BytesIO(wav)) as audio:
            duration = audio.getnframes() / audio.getframerate()
            drop = audio.getnframes() == 4 * audio.getframerate() and not any(audio.readframes(audio.getnframes()))
        key = hashlib.sha256(wav).digest()
        self.requests.append((duration, transferred))
        self.payloads.append((key.hex(), drop))
        if self.prepare:
            self.inputs[key] = wav
            return {"text": "", "segments": []}
        if self.answers is not None:
            return self.answers[key]
        return self.infer(wav)

    def infer(self, wav):
        from faster_whisper.audio import decode_audio
        audio = decode_audio(io.BytesIO(wav), sampling_rate=16000)
        segments, _ = self.model.transcribe(audio, language="en", beam_size=5, vad_filter=True,
                                            condition_on_previous_text=False)
        rows = [{"text": s.text, "start": s.start, "end": s.end} for s in segments]
        return {"text": " ".join(row["text"].strip() for row in rows), "segments": rows}

    def freeze(self):
        self.prepare = False
        self.answers = {}
        for index, (digest, wav) in enumerate(self.inputs.items(), 1):
            self.answers[digest] = self.infer(wav)
            print(f"Recognized unique WAV {index}/{len(self.inputs)}", flush=True)
        self.inputs.clear()


def server(recognizer):
    class LocalEndpoint(BaseHTTPRequestHandler):
        def do_POST(self):
            try:
                if self.path != "/v1/audio/transcriptions":
                    raise ValueError("Wrong test route")
                size = int(self.headers["Content-Length"])
                if not 0 < size <= 2 * 1024 * 1024:
                    raise ValueError("Unexpected test request size")
                data = self.rfile.read(size)
                mime = ("MIME-Version: 1.0\r\nContent-Type: " + self.headers["Content-Type"] + "\r\n\r\n").encode()
                parts = BytesParser(policy=policy.default).parsebytes(mime + data)
                media = [p for p in parts.iter_parts() if p.get_param("name", header="content-disposition") == "file"]
                if len(media) != 1:
                    raise ValueError("Missing PCM attachment")
                started = time.monotonic()
                response = recognizer.recognize(media[0].get_payload(decode=True), len(data))
                recognizer.service_seconds.append(time.monotonic() - started)
                body = json.dumps(response).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                return
            except Exception as exc:
                self.send_error(500, str(type(exc).__name__))

        def log_message(self, *args):
            pass

    http = ThreadingHTTPServer(("127.0.0.1", 0), LocalEndpoint)
    thread = threading.Thread(target=http.serve_forever, daemon=True)
    thread.start()
    return http


def score_case(case, transcripts):
    stats = Counter()
    matched_boundaries = []
    for source, text in transcripts.items():
        ref, onsets, tails = [], [], []
        for turn in case["turns"]:
            if turn["track"] == source:
                chunk = words(turn["text"])
                if turn["quiet"]:
                    onsets.append(len(ref))
                    tails.append(len(ref) + len(chunk) - 1)
                ref.extend(chunk)
        errors, matched = word_alignment(ref, words(text))
        stats.update(errors)
        stats["reference_words"] += len(ref)
        quiet = onsets + tails
        stats["quiet_boundaries"] += len(quiet)
        stats["quiet_onsets_recovered"] += sum(i in matched for i in onsets)
        stats["quiet_tails_recovered"] += sum(i in matched for i in tails)
        stats["quiet_boundaries_recovered"] += sum(i in matched for i in quiet)
        matched_boundaries.extend(f"{source}:{i}" for i in quiet if i in matched)
    return {**stats, "matched_quiet_boundaries": matched_boundaries}


def all_speech(corpus, case):
    # Use consecutive complete utterances from the annotated program and mic,
    # excluding the overlapping exchange to create a dense one-source control.
    parts = []
    refs = []
    for i in (0, 1, 4, 5):
        turn = case["turns"][i]
        source = samples48(corpus / case["case"] / (turn["track"] + ".wav"))
        first, end = turn["start_ms"] * 96, turn["end_ms"] * 96
        parts.append(source[first:end])
        refs.append(dict(turn, track="program"))
    return {"case": "dense", "turns": refs}, {"program": b"".join(parts)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worker", required=True, type=Path)
    parser.add_argument("--model", required=True, type=Path)
    parser.add_argument("--corpus", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--trials", default=5, type=int)
    parser.add_argument("--speed", default=1.0, type=float)
    parser.add_argument("--precompute", action="store_true",
                        help="Freeze real ASR answers by exact worker WAV hash before testing. Excludes endpoint latency.")
    parser.add_argument("--warmup", type=Path, help="Run one local synthetic WAV through ASR before serving requests")
    check = parser.add_mutually_exclusive_group()
    check.add_argument("--verify-zero-cores", action="store_true",
                        help="Require identical source positions and WAV bytes for every retained request")
    check.add_argument("--verify-guarded-cores", action="store_true",
                       help="Also retain zero cores within 240 ms after or 160 ms before nonzero samples")
    args = parser.parse_args()
    if args.speed <= 0 or args.trials <= 0 or not (args.model / "model.bin").is_file():
        parser.error("Use positive trials and speed, and an installed model directory.")
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    cases = offline.read(args.corpus / "corpus.json")["cases"]
    recognizer = LocalRecognizer(args.model)
    if args.warmup is not None:
        recognizer.infer(args.warmup.read_bytes())
    http = server(recognizer)
    url = f"http://127.0.0.1:{http.server_port}/v1"
    report = {"worker_sha256": offline.digest(args.worker), "model_sha256": offline.digest(args.model / "model.bin"),
              "corpus_sha256": offline.digest(args.corpus / "corpus.json"), "speed": args.speed,
              "precomputed_asr": args.precompute, "results": []}
    def fixtures():
        for case in cases:
            yield case, {t: samples48(args.corpus / case["case"] / (t + ".wav")) for t in ("program", "mic")}
        yield all_speech(args.corpus, cases[0])
    try:
        if args.precompute:
            recognizer.prepare = True
            for case, samples in fixtures():
                for gated in (False, True):
                    paced_worker(args.worker.resolve(), url, root / f'prepare-{case["case"]}-{int(gated)}',
                                 samples, gated, args.speed)
            recognizer.freeze()
        for trial in range(1, args.trials + 1):
            for case, samples in fixtures():
                compared = {}
                for gated in ((False, True) if trial % 2 else (True, False)):
                    recognizer.requests.clear()
                    recognizer.payloads.clear()
                    recognizer.service_seconds.clear()
                    ranges = []
                    heard, size = paced_worker(args.worker.resolve(), url,
                                               root / f'{trial}-{case["case"]}-{int(gated)}',
                                               samples, gated, args.speed, frame_ranges=ranges)
                    compared[gated] = list(zip(ranges, recognizer.payloads, strict=True))
                    counts = score_case(case, heard)
                    row = {"trial": trial, "case": case["case"], "gated": gated, **counts,
                           "source_seconds": sum(len(b) / 96000 for b in samples.values()),
                           "asr_seconds": sum(s for s, _ in recognizer.requests),
                           "service_seconds": list(recognizer.service_seconds),
                           "multipart_bytes": sum(b for _, b in recognizer.requests),
                           "request_count": len(recognizer.requests), "txt_bytes": size}
                    report["results"].append(row)
                    offline.write(root / "scores.json", report)
                    print(row, flush=True)
                if args.verify_zero_cores or args.verify_guarded_cores:
                    keep = []
                    for r, (key, drop) in compared[False]:
                        source, first, end = r
                        if drop and args.verify_guarded_cores:
                            pcm = samples[source]
                            before = pcm[max(0, first - 11520) * 2:first * 2]
                            after = pcm[end * 2:(end + 7680) * 2]
                            drop = not (any(before) or any(after))
                        if not drop:
                            keep.append((r, key))
                    actual = [(r, k) for r, (k, _) in compared[True]]
                    if sorted(keep) != sorted(actual):
                        raise RuntimeError("Gating changed a retained WAV or its source position.")
                    row["retained_payloads_and_positions_match"] = True
                    offline.write(root / "scores.json", report)
    finally:
        http.shutdown()
        http.server_close()


if __name__ == "__main__":
    main()
