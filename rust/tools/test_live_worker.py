"""Test an extracted Windows worker with generated audio and loopback HTTP."""

import argparse
from contextlib import contextmanager
from dataclasses import dataclass
from email import policy
from email.parser import BytesParser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
from queue import Empty, Queue
import re
import struct
import subprocess
import tempfile
from threading import Event, Thread
from time import monotonic
import uuid
import wave


RATE = 48_000
PACKET_FRAMES = 4_800
DISCORD = "Discord"
MIC = "Microphone – USB"
PROGRAM_TEXT = "Program says go go."
MIC_TEXT = "Microphone replies café."


def encode_control(kind, **fields):
    payload = json.dumps({kind: fields}, ensure_ascii=False).encode("utf-8")
    return struct.pack("<4sBI", b"ACL1", 0, len(payload)) + payload


def encode_audio(source, first, pcm_bytes):
    if len(pcm_bytes) % 2 != 0 or not 0 < len(pcm_bytes) <= PACKET_FRAMES * 2:
        raise ValueError("Expected up to 4,800 complete PCM16 samples")
    payload = struct.pack("<HQ", source, first) + pcm_bytes
    return struct.pack("<4sBI", b"ACL1", 1, len(payload)) + payload


def wav_bytes(sample, frames):
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as output:
        output.setparams((1, 2, RATE, 0, "NONE", "not compressed"))
        output.writeframes(struct.pack("<h", sample) * frames)
    return buffer.getvalue()


@dataclass(frozen=True)
class Request:
    model: str
    frames: int
    rate: int
    first_sample: int


class MockASR:
    """Accept bounded PCM WAV requests on an ephemeral local port."""

    def __init__(self, port=0, response=None):
        self.requests = []
        self.errors = []
        self.response = response or self.transcribe
        server = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                try:
                    if self.path != "/test/v1/audio/transcriptions":
                        raise ValueError("Transcription request has the wrong route")
                    size = int(self.headers["Content-Length"])
                    if not 0 < size < 2 * 1024 * 1024:
                        raise ValueError("Transcription request exceeds the fixture size limit")
                    self.connection.settimeout(5)
                    raw = self.rfile.read(size)
                    mime = f'MIME-Version: 1.0\r\nContent-Type: {self.headers["Content-Type"]}\r\n\r\n'
                    message = BytesParser(policy=policy.default).parsebytes(mime.encode() + raw)
                    fields = {
                        item.get_param("name", header="content-disposition"):
                        item.get_payload(decode=True)
                        for item in message.iter_parts()
                    }
                    with wave.open(io.BytesIO(fields["file"]), "rb") as data:
                        assert data.getnchannels() == 1 and data.getsampwidth() == 2
                        start = data.readframes(1)
                        req = Request(
                            fields["model"].decode("utf-8"),
                            data.getnframes(),
                            data.getframerate(),
                            struct.unpack("<h", start)[0] if start else 0,
                        )
                    server.requests.append(req)
                    result = server.response(req)
                    if result is None:
                        self.close_connection = True
                        return
                    payload = json.dumps(result, ensure_ascii=False).encode("utf-8")
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                except (BrokenPipeError, ConnectionResetError):
                    pass
                except Exception as error:
                    server.errors.append(repr(error))
                    self.send_error(500, "Mock transcription failed")

            def log_message(self, _format, *_args):
                pass

        self.httpd = ThreadingHTTPServer(("127.0.0.1", port), Handler)
        self.url = f"http://127.0.0.1:{self.httpd.server_port}/test/v1"
        self.thread = Thread(target=self.httpd.serve_forever, args=(0.05,), daemon=True)

    @staticmethod
    def transcribe(request):
        text = PROGRAM_TEXT if request.first_sample >= 0 else MIC_TEXT
        return {"text": text}

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, exc_type, _value, _traceback):
        self.httpd.shutdown()
        self.httpd.server_close()
        self.thread.join(timeout=2)
        if exc_type is None:
            assert not self.errors, self.errors


class Harness:
    def __init__(self, executable, root):
        self.worker = executable
        self.root = root
        self.local = root / "Local profile"
        self.local.mkdir()
        self.env = dict(os.environ, LOCALAPPDATA=str(self.local))

    def run(self, *args, expected=0, answer=None):
        result = subprocess.run(
            [self.worker, *args], cwd=self.root, env=self.env, input=answer,
            capture_output=True, encoding="utf-8", timeout=40,
        )
        assert result.returncode == expected, (args, result.returncode, result.stdout, result.stderr)
        return result.stdout

    @contextmanager
    def child(self, *args):
        proc = subprocess.Popen(
            [self.worker, *args], cwd=self.root, env=self.env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        lines = Queue()

        def read_stdout():
            for line in proc.stdout:
                lines.put(line.decode("utf-8", errors="replace").strip())
            lines.put(None)

        reader = Thread(target=read_stdout, daemon=True)
        reader.start()
        try:
            yield Child(proc, lines)
        finally:
            if proc.poll() is None:
                proc.kill()
            proc.wait(timeout=5)
            reader.join(timeout=5)
            for stream in [proc.stdin, proc.stdout, proc.stderr]:
                stream.close()

    def live(self, endpoint, output, sources, model="fixture"):
        source_args = [word for name in sources for word in ["--source", name]]
        return self.child(
            "live", "--confirmed", "--base-url", endpoint.url, "--model", model,
            "--output-dir", output, *source_args,
        )


@dataclass
class Child:
    proc: subprocess.Popen
    lines: Queue

    def send(self, wire):
        self.proc.stdin.write(wire)
        self.proc.stdin.flush()

    def text(self, predicate, timeout=25):
        seen = []
        deadline = monotonic() + timeout
        while monotonic() < deadline:
            try:
                line = self.lines.get(timeout=max(0, deadline - monotonic()))
            except Empty:
                break
            if line is None:
                error = self.proc.stderr.read().decode("utf-8", errors="replace")
                raise AssertionError(f"Worker exited before expected output. {seen}\n{error}")
            if predicate(line):
                return line
            seen.append(line)
        raise AssertionError(f"Timed out waiting for worker output. Last lines: {seen[-8:]}")

    def state(self, wanted, test=None):
        def matches(line):
            row = json.loads(line)
            return row.get("state") == wanted and (test is None or test(row))

        return json.loads(self.text(matches))

    def done(self, expected=0):
        if not self.proc.stdin.closed:
            self.proc.stdin.close()
        code = self.proc.wait(timeout=35)
        error = self.proc.stderr.read().decode("utf-8", errors="replace")
        assert code == expected, (expected, code, error)
        return error


def start(child, interval, sources, first=0):
    child.send(encode_control("Start", session="fixture-session", interval=interval,
                              first=first, sources=sources))
    return Path(child.state("active")["path"])


def pcm(child, source, first, end, sample):
    for offset in range(first, end, PACKET_FRAMES):
        sample_count = min(end - offset, PACKET_FRAMES)
        child.send(encode_audio(source, offset, struct.pack("<h", sample) * sample_count))


def index_for(output):
    path = Path(output).with_suffix(".segments.jsonl")
    rows = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]
    assert [row["sequence"] for row in rows] == sorted({row["sequence"] for row in rows})
    return path, rows


def streaming(harness, root):
    output = root / "Live output ü"
    output.mkdir()
    interval = "test-" + uuid.uuid4().hex
    collision = output / f"Transcript-{interval}.txt"
    collision.write_bytes(b"Leave this output alone.")
    with MockASR() as endpoint:
        rejected = harness.run(
            "live", "--base-url", endpoint.url, "--model", "fixture",
            "--output-dir", output, "--source", DISCORD, expected=1,
        )
        assert all(json.loads(line).get("state") == "error" for line in rejected.splitlines())
        assert endpoint.requests == []
        with harness.live(endpoint, output, [DISCORD]) as denied:
            denied.state("ready")
            denied.send(encode_control("Start", session="denied-session", interval="denied",
                                       first=0, sources=[MIC]))
            assert "scope" in denied.state("error")["message"]
            denied.done(expected=1)
        assert endpoint.requests == []
        with harness.live(endpoint, output, [DISCORD, MIC]) as child:
            child.state("ready")
            assert endpoint.requests == []
            first = RATE
            partial = start(child, interval, [DISCORD, MIC], first)
            assert partial.is_file() and partial.name.endswith(".partial.txt")
            middle = first + RATE * 4
            pcm(child, 0, first, middle, 1000)
            pcm(child, 1, first, middle, -1000)
            child.state("active", lambda row: MIC_TEXT in (row.get("preview") or ""))
            draft = partial.read_text(encoding="utf-8")
            assert PROGRAM_TEXT in draft and MIC_TEXT in draft, draft
            assert not list(output.glob("*.segments.jsonl"))
            end = middle + RATE // 2
            pcm(child, 0, middle, end, 1000)
            child.send(encode_control("Gap", source=1, first=middle, end=end))
            child.send(encode_control("Recording", id="sample-recording", start=first, end=end))
            child.send(encode_control("End", end=end))
            child.state("saving")
            final = Path(child.state("incomplete")["path"])
            child.done()
        assert len(endpoint.requests) == 3, endpoint.requests
        assert all(req.rate == RATE for req in endpoint.requests)
        assert sorted(req.frames for req in endpoint.requests) == [RATE // 2, RATE * 4, RATE * 4]
        assert sum(req.first_sample < 0 for req in endpoint.requests) == 1
        assert final != collision and collision.read_bytes() == b"Leave this output alone."
        text = final.read_text(encoding="utf-8")
        assert text.count(PROGRAM_TEXT) == 2 and text.count(MIC_TEXT) == 1
        assert DISCORD in text and MIC in text
        _, rows = index_for(final)
        speech = [row for row in rows if row["type"] == "speech"]
        assert len({row["id"] for row in speech}) == 3
        assert {(row["source"], row["start_frame"], row["end_frame"]) for row in speech} == {
            (DISCORD, first, middle), (MIC, first, middle), (DISCORD, middle, end),
        }
        gaps = [row for row in rows if row["type"] == "gap"]
        assert [(row["source"], row["start_frame"], row["end_frame"]) for row in gaps] == [(MIC, middle, end)]
        assert any(row["type"] == "recording" and row["id"] == "sample-recording" for row in rows)
        assert rows[-1]["type"] == "end" and rows[-1]["cutoff_frame"] == end
        assert not list(root.rglob("*.wav")), "Transcript-only run left behind WAV audio"


def recover(harness, root):
    output = root / "Recovered output"
    output.mkdir()
    interval = "recovery-" + uuid.uuid4().hex
    second_request = Event()
    release_request = Event()

    def recognize(request):
        if request.first_sample < 0:
            second_request.set()
            release_request.wait(timeout=15)
            return None
        return MockASR.transcribe(request)

    with MockASR(response=recognize) as endpoint:
        with harness.live(endpoint, output, [DISCORD, MIC]) as child:
            child.state("ready")
            partial = start(child, interval, [DISCORD, MIC])
            pcm(child, 0, 0, RATE * 4, 1000)
            child.state("active", lambda row: PROGRAM_TEXT in (row.get("preview") or ""))
            pcm(child, 1, 0, RATE * 4, -1000)
            try:
                assert second_request.wait(10), "Second request never reached the endpoint"
                child.proc.kill()
                child.proc.wait(timeout=5)
            finally:
                release_request.set()
        assert PROGRAM_TEXT in partial.read_text(encoding="utf-8")
        assert interval in harness.run("recover-live", "list")
        # Simulate a torn UTF-8 append after the last durable checkpoint.
        with partial.open("ab") as tail:
            tail.write(b"torn \xe2\x82")
        first = Path(harness.run("recover-live", "publish", interval).strip())
        text = first.read_bytes()
        assert PROGRAM_TEXT.encode() in text and b"torn" not in text
        assert len(endpoint.requests) == 2, endpoint.requests
        index_path, events = index_for(first)
        index = index_path.read_bytes()
        assert str(first) == harness.run("recover-live", "publish", interval).strip()
        assert first.read_bytes() == text and index_path.read_bytes() == index
        assert any(row["type"] == "gap" and row["source"] == MIC for row in events)
        assert events[-1]["incomplete"]
        assert interval not in harness.run("recover-live", "list")
        assert len(endpoint.requests) == 2, "Recovery sent another request"
        assert not list(root.rglob("*.wav")), "Recovery left temporary WAV audio"


def destination_loss(harness, root):
    output = root / "Original output"
    output.mkdir()
    recovered = root / "Retry output"
    interval = "destination-loss"
    with MockASR() as endpoint:
        with harness.live(endpoint, output, [DISCORD]) as child:
            child.state("ready")
            partial = start(child, interval, [DISCORD])
            pcm(child, 0, 0, RATE // 10, 1000)
            output.rename(root / "Unavailable")
            output.write_bytes(b"Leave this unrelated file alone.")
            child.send(encode_control("End", end=RATE // 10))
            child.state("error")
            child.done(expected=1)
        assert PROGRAM_TEXT in partial.read_text(encoding="utf-8")
        assert interval in harness.run("recover-live", "list")
        final = Path(harness.run("recover-live", "publish", interval, "--output-dir", recovered).strip())
        assert final.parent.resolve() == recovered.resolve()
        assert PROGRAM_TEXT in final.read_text(encoding="utf-8")
        assert output.read_bytes() == b"Leave this unrelated file alone."
        assert len(endpoint.requests) == 1


def batch(harness, root):
    first = root / "batch-one.wav"
    second = root / "batch-two.wav"
    first.write_bytes(wav_bytes(1000, 4800))
    second.write_bytes(wav_bytes(-1000, 4800))
    batch_out = root / "Batch output"
    live_out = root / "Parallel transcription"
    batch_out.mkdir()
    live_out.mkdir()
    received = Event()
    release = Event()

    def recognize(request):
        if request.model == "held-batch" and request.first_sample > 0:
            received.set()
            release.wait(timeout=20)
        return MockASR.transcribe(request)

    with MockASR(response=recognize) as endpoint:
        # Blank API key, separate timelines, no timestamps, no splitting or review,
        # then approve the exact files and the isolated loopback endpoint.
        answers = f"{endpoint.url}\nheld-batch\n\n{batch_out}\nn\nn\nn\nn\ny\n"
        cancelled = harness.run("--guided", "--", first, second, answer=answers[:-2] + "n\n")
        assert "Cancelled" in cancelled and endpoint.requests == []
        with harness.child("--guided", "--", first, second) as driver:
            driver.send(answers.encode("utf-8"))
            batch_line = driver.text(lambda line: "Batch ID: " in line)
            batch_id = re.search(r"Batch ID: (batch_[0-9a-f]{32})", batch_line)[1]
            try:
                assert received.wait(10), "Guided batch did not reach the loopback endpoint"
                with harness.live(endpoint, live_out, [DISCORD]) as live:
                    live.state("waiting")
                    assert len(endpoint.requests) == 1
                    harness.run("batch-queue", "pause", batch_id)
                    release.set()
                    live.state("ready")
                    start(live, "batch-coexistence", [DISCORD])
                    pcm(live, 0, 0, RATE // 10, -1000)
                    live.send(encode_control("End", end=RATE // 10))
                    final = Path(live.state("complete")["path"])
                    live.done()
                    assert MIC_TEXT in final.read_text(encoding="utf-8")
                driver.done()
            finally:
                release.set()
        assert len(endpoint.requests) == 2, endpoint.requests
        harness.run("batch-queue", "resume", batch_id)
        assert [req.model for req in endpoint.requests] == ["held-batch", "fixture", "held-batch"]
        status = harness.run("batch-queue", "status", batch_id)
        assert len(re.findall(r"completed", status, re.IGNORECASE)) == 2, status
        assert [p.read_bytes() for p in [first, second]] == [
            wav_bytes(1000, 4800), wav_bytes(-1000, 4800),
        ]
        duplicate = root / "same audio under another name.wav"
        duplicate.write_bytes(second.read_bytes())
        one_file_answers = f"{endpoint.url}\nheld-batch\n\n{batch_out}\nn\nn\nn\ny\n"
        cached = harness.run("--guided", "--", duplicate, answer=one_file_answers)
        assert "Transcription complete" in cached
        assert len(endpoint.requests) == 3, "Cached result triggered another request"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    cases = {case.__name__: case for case in (streaming, recover, destination_loss, batch)}
    parser.add_argument("worker", type=Path, nargs="?", help="Extracted transcribe.exe")
    parser.add_argument("--serve", action="store_true", help="Start a loopback mock for GUI tests")
    parser.add_argument("--port", type=int, default=0, help="Loopback port, or zero for an available port")
    parser.add_argument("--case", action="append", choices=list(cases), help="Run one named check")
    options = parser.parse_args()
    if options.serve:
        with MockASR(options.port) as endpoint:
            print(json.dumps({"url": endpoint.url, "model": "fixture"}), flush=True)
            try:
                endpoint.thread.join()
            except KeyboardInterrupt:
                pass
        return
    if options.worker is None:
        parser.error("Supply an extracted worker or select --serve")
    worker = options.worker.resolve(strict=True)
    for name in options.case or cases:
        with tempfile.TemporaryDirectory(prefix=f"audio-capture-live-{name}-") as directory:
            root = Path(directory)
            cases[name](Harness(worker, root), root)
        print(f"Packaged worker {name} passed.")


if __name__ == "__main__":
    main()
