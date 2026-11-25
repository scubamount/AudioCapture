"""Check native video transcription with generated MP4s and a loopback endpoint."""

import argparse
from array import array
from contextlib import contextmanager
from email import policy
from email.parser import BytesParser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tempfile
from threading import Thread
import wave


MIB = 1024 * 1024
RATE = 16_000
MODEL = "fixture-video"
ROUTE = "/fixture/v1/audio/transcriptions"


@contextmanager
def endpoint():
    requests = []
    errors = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            try:
                assert self.path == ROUTE, self.path
                length = int(self.headers["Content-Length"])
                assert 0 < length <= MIB + 65_536, "Request exceeds the 1 MiB WAV limit"
                self.connection.settimeout(10)
                headers = f'MIME-Version: 1.0\r\nContent-Type: {self.headers["Content-Type"]}\r\n\r\n'
                mime = BytesParser(policy=policy.default).parsebytes(headers.encode() + self.rfile.read(length))
                fields = {
                    part.get_param("name", header="content-disposition"): part for part in mime.iter_parts()
                }
                assert fields["model"].get_payload(decode=True) == MODEL.encode()
                assert fields["response_format"].get_payload(decode=True) == b"verbose_json"
                file = fields["file"]
                payload = file.get_payload(decode=True)
                assert file.get_filename() == "recording.wav", (
                    f"Expected PCM WAV, received {file.get_filename()} with header {payload[:12].hex()}"
                )
                assert len(payload) <= MIB and payload[:4] == b"RIFF" and payload[8:12] == b"WAVE"
                with wave.open(io.BytesIO(payload), "rb") as audio:
                    assert (audio.getnchannels(), audio.getsampwidth(), audio.getframerate()) == (1, 2, RATE)
                    frames = audio.getnframes()
                    pcm = array("h", audio.readframes(frames))
                assert len(pcm) == frames and frames > 0
                probe = pcm[RATE // 5:2 * RATE // 5]
                crossings = sum(1 for index in range(1, len(probe)) if probe[index - 1] <= 0 < probe[index])
                assert max(probe, default=0) >= 500 and 80 <= crossings <= 97, (
                    f"Missing or altered 440 Hz audio ({frames} frames, {crossings} crossings, "
                    f"peak {max(probe, default=0)})"
                )
                requests.append(frames)
                text = f"Recognized video part {len(requests)}."
                body = json.dumps({
                    "text": text, "segments": [{"start": 0.05, "end": 0.12, "text": text}],
                }).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            except (AssertionError, KeyError, ValueError, OSError, wave.Error) as error:
                errors.append(str(error))
                self.send_error(400, "Fixture expected a bounded audio-only WAV")

        def log_message(self, _format, *_args):
            pass

    with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        thread = Thread(target=server.serve_forever, args=(0.05,), daemon=True)
        thread.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}/fixture/v1", requests, errors
        finally:
            server.shutdown()
            thread.join(timeout=2)


def make_video(ffmpeg, output, seconds, audio_tracks, *, flags=()):
    inputs = ["-f", "lavfi", "-i", f"color=size=64x64:rate=1:duration={seconds}"]
    maps = ["-map", "0:v:0"]
    for index, frequency in enumerate((440, 880)[:audio_tracks], start=1):
        inputs.extend(["-f", "lavfi", "-i", f"sine=frequency={frequency}:sample_rate=48000:duration={seconds}"])
        maps.extend(["-map", f"{index}:a:0"])
    audio = ["-c:a", "aac", "-ac", "2"] if audio_tracks else ["-an"]
    subprocess.run([
        ffmpeg, "-nostdin", "-v", "error", *inputs, *maps,
        "-pix_fmt", "yuv420p", "-c:v", "mpeg4", *audio, *flags, output,
    ], check=True, timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)


def check_manifest(output, original, frame_counts):
    manifest_path, = output.glob(f"{original.stem}-*-combined.json")
    manifest = json.loads(manifest_path.read_bytes())
    assert Path(manifest["source"]).samefile(original), manifest["source"]
    assert manifest["source_sha256"] == hashlib.sha256(original.read_bytes()).hexdigest()
    assert manifest["audio_extraction"] == "windows-media-foundation-pcm16-v1"
    assert manifest["sample_rate"] == RATE
    assert [part["frames"] for part in manifest["parts"]] == frame_counts
    offset = 0
    for part in manifest["parts"]:
        assert part["offset_frames"] == offset, part
        assert part["segments"][0]["start"] == 0.05, part
        offset += part["frames"]
    assert not list(output.rglob("*.wav")), "Temporary WAV files survived the job"
    return manifest_path


def check_review(review, original, frame_counts):
    embedded = re.search(r"^const tracks = (.*);$", review.read_text(encoding="utf-8"), re.MULTILINE)
    assert embedded is not None
    track, = json.loads(embedded[1])
    assert track["label"] == original.stem, track["label"]
    assert len(track["segments"]) == len(frame_counts)
    offset = 0
    for segment, count in zip(track["segments"], frame_counts, strict=True):
        assert abs(segment["start"] - (offset / RATE + 0.05)) < 1e-6, segment
        assert abs(segment["end"] - (offset / RATE + 0.12)) < 1e-6, segment
        offset += count


def run_tests(worker, ffmpeg, root):
    local = root / "local"
    local.mkdir()
    temp = root / "temp"
    temp.mkdir()
    package = root / "portable worker"
    package.mkdir()
    worker = Path(shutil.copyfile(worker, package / "transcribe.exe"))
    env = {k: v for k, v in os.environ.items() if not k.upper().startswith("AUDIOCAPTURE_")}
    env.update(PATH="", LOCALAPPDATA=str(local), APPDATA=str(local), TEMP=str(temp), TMP=str(temp))

    def launch(*args, answer=""):
        return subprocess.run([worker, *args], cwd=root, env=env, input=answer,
                              capture_output=True, encoding="utf-8", timeout=50,
                              creationflags=subprocess.CREATE_NO_WINDOW)

    short = root / "demo audio é.MP4"
    long = root / "long demo.mp4"
    video_only = root / "video only.mp4"
    for path, seconds, audio_tracks in [(short, 2, 2), (long, 35, 1), (video_only, 2, 0)]:
        make_video(ffmpeg, path, seconds, audio_tracks)
        assert path.stat().st_size < MIB, path
    padded = root / "large container.mp4"
    padding = struct.pack(">I4s", MIB + 8, b"free") + bytes(MIB)
    padded.write_bytes(short.read_bytes() + padding)
    fragmented = root / "fragmented second track default.mp4"
    make_video(ffmpeg, fragmented, 2, 2, flags=(
        "-disposition:a:0", "0", "-disposition:a:1", "default",
        "-movflags", "+frag_keyframe+empty_moov+default_base_moof",
    ))
    invalid = root / "invalid.mp4"
    invalid.write_bytes(b"not an MP4 file")
    before = {path: hashlib.sha256(path.read_bytes()).hexdigest()
              for path in (short, long, video_only, padded, fragmented, invalid)}

    with endpoint() as (url, requests, errors):
        def explicit(output, *paths, review=None):
            review_args = ["--review", review] if review else []
            return launch("--base-url", url, "--model", MODEL, "--timestamps", "--max-file-mib", "1",
                          "--output-dir", output, *review_args, "--", *paths)

        out = root / "short transcript"
        result = explicit(out, short)
        assert result.returncode == 0 and not errors, (errors, result.stdout, result.stderr)
        assert len(requests) == 1 and abs(requests[0] - 2 * RATE) < RATE // 20, requests
        manifest = check_manifest(out, short, requests)
        saved = manifest.read_bytes()
        result = explicit(out, short)
        assert result.returncode == 0 and len(requests) == 1 and manifest.read_bytes() == saved, result.stderr

        offset = len(requests)
        out = root / "fragmented transcript"
        result = explicit(out, fragmented)
        assert result.returncode == 0 and not errors, (errors, result.stdout, result.stderr)
        parts = requests[offset:]
        assert len(parts) == 1 and abs(parts[0] - 2 * RATE) < RATE // 20, parts
        check_manifest(out, fragmented, parts)

        offset = len(requests)
        out = root / "large container transcript"
        result = explicit(out, padded)
        assert result.returncode == 0 and not errors, (errors, result.stdout, result.stderr)
        assert requests[offset:] == requests[:1], requests
        check_manifest(out, padded, requests[offset:])

        offset = len(requests)
        out = root / "long transcript"
        review = root / "long review.html"
        result = explicit(out, long, review=review)
        assert result.returncode == 0 and not errors, (errors, result.stdout, result.stderr)
        parts = requests[offset:]
        assert len(parts) == 2 and parts[0] == (MIB - 44) // 2, parts
        assert abs(sum(parts) - 35 * RATE) < RATE // 20, parts
        check_manifest(out, long, parts)
        check_review(review, long, parts)
        cached_review = root / "cached long review.html"
        result = explicit(out, long, review=cached_review)
        assert result.returncode == 0 and len(requests) == offset + 2, result.stderr
        check_review(cached_review, long, parts)

        offset = len(requests)
        out = root / "video-only transcript"
        result = explicit(out, video_only)
        assert result.returncode != 0 and len(requests) == offset and not errors, (errors, result.stderr)
        assert "audio" in result.stderr.lower() and not list(out.glob("*.txt")), result.stderr

        out = root / "invalid transcript"
        result = explicit(out, invalid)
        assert result.returncode != 0 and len(requests) == offset and not errors, (errors, result.stderr)
        assert invalid.name in result.stderr and not list(out.glob("*.txt")), result.stderr

        guided_out = root / "guided transcript"
        (package / "transcription.json").write_text(json.dumps({
            "version": 1, "endpoint": url, "model": MODEL, "api_key_env": "",
            "transcript_output_folder": str(guided_out), "request_segment_timestamps": True,
            "speech_gating": False, "speaker_labels": False, "speaker_adapter_config": "",
        }), encoding="utf-8")
        result = launch("--guided", "--", short, answer="n\nn\n")
        assert result.returncode == 0 and "Cancelled" in result.stdout and len(requests) == offset, result
        assert "first audio stream" in result.stdout and short.name in result.stdout, result.stdout
        assert "ffmpeg" not in result.stdout.lower(), result.stdout
        result = launch("--guided", "--", short, answer="n\ny\n")
        assert result.returncode == 0 and len(requests) == offset + 1 and not errors, (
            errors, result.stdout, result.stderr,
        )
        check_manifest(guided_out, short, requests[offset:])

        offset = len(requests)
        mixed_out = root / "mixed transcript"
        result = explicit(mixed_out, video_only, short)
        assert result.returncode != 0 and len(requests) == offset + 1 and not errors, (
            errors, result.stdout, result.stderr,
        )
        assert video_only.name in result.stderr and "1 item(s) failed" in result.stderr, result.stderr
        check_manifest(mixed_out, short, requests[offset:])

    assert all(hashlib.sha256(path.read_bytes()).hexdigest() == digest for path, digest in before.items())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("worker", type=Path)
    options = parser.parse_args()
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        parser.error("FFmpeg is required only to generate fixtures")
    with tempfile.TemporaryDirectory(prefix="audio-capture-video-smoke-") as directory:
        run_tests(options.worker.resolve(strict=True), ffmpeg, Path(directory))
    print("Native MP4 extraction passed for short, bounded long, video-only, mixed and guided jobs.")


if __name__ == "__main__":
    main()
