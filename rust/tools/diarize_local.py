"""Run an installed sherpa-onnx diarizer for one local source on request.

Install sherpa-onnx and NumPy in a separate Python environment. Install FFmpeg
and supply local ONNX segmentation and speaker embedding files. See the sherpa
speaker diarization guide for compatible models and their individual terms.
https://k2-fsa.github.io/sherpa/onnx/speaker-diarization/

Invoke this script through the worker's explicit adapter configuration.
It reads one UTF-8 JSON request from stdin and writes one JSON reply to stdout.
It makes no web requests. The process exits after each source.
"""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys


def parse_options():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--segmentation", type=Path, required=True)
    parser.add_argument("--embedding", type=Path, required=True)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--threads", type=int, default=2)
    parser.add_argument("--threshold", type=float, default=0.5)
    parser.add_argument("--max-seconds", type=int, default=3600)
    options = parser.parse_args()
    if not 1 <= options.threads <= 32 or not 0 < options.threshold < 2:
        parser.error("Use 1 to 32 threads and a clustering threshold between 0 and 2.")
    if not 1 <= options.max_seconds <= 14_400:
        parser.error("Use 1 to 14400 seconds for the maximum source length.")
    if not all(path.is_file() for path in [options.segmentation, options.embedding]):
        parser.error("Both ONNX model files must exist locally.")
    return options


def request(max_seconds):
    raw = sys.stdin.buffer.read(65537)
    if len(raw) > 65536:
        raise ValueError("Adapter request exceeds 64 KiB.")
    data = json.loads(raw.decode("utf-8"))
    if set(data) != {"protocol_version", "source_id", "audio_path", "duration_ms"}:
        raise ValueError("Unexpected request fields.")
    if type(data["protocol_version"]) is not int or data["protocol_version"] != 1:
        raise ValueError("Unsupported adapter protocol version.")
    if not isinstance(data["source_id"], str) or not data["source_id"]:
        raise ValueError("Missing source ID.")
    duration = data["duration_ms"]
    if type(duration) is not int or not 0 < duration <= max_seconds * 1000:
        raise ValueError("Source duration is missing or exceeds the configured limit.")
    path = Path(data["audio_path"])
    if not path.is_absolute() or not path.is_file():
        raise ValueError("Provide an absolute path to an existing audio file.")
    return data


def decode(options, data, rate, numpy):
    # Restrict FFmpeg to local media demuxers. The extra 100 ms lets us reject
    # an understated duration without reading an unbounded decoded stream.
    command = [
        options.ffmpeg, "-nostdin", "-v", "error", "-protocol_whitelist", "file,pipe",
        "-format_whitelist", "aac,aiff,asf,flac,matroska,webm,mov,mp3,mpeg,mpegts,ogg,wav",
        "-i", data["audio_path"], "-map", "0:a:0", "-vn", "-sn", "-dn",
        "-t", f'{data["duration_ms"] / 1000 + 0.1:.3f}',
        "-ar", str(rate), "-ac", "1", "-f", "f32le", "pipe:1",
    ]
    pcm = subprocess.run(command, stdout=subprocess.PIPE, check=True).stdout
    samples = numpy.frombuffer(pcm, dtype="<f4")
    actual_ms = round(len(samples) * 1000 / rate)
    if not len(samples) or abs(actual_ms - data["duration_ms"]) > 50:
        raise ValueError("Decoded audio duration differs from the supplied duration.")
    return samples, min(actual_ms, data["duration_ms"])


def model_digest(path):
    with path.open("rb") as model:
        return hashlib.file_digest(model, "sha256").hexdigest()


def main():
    options = parse_options()
    data = request(options.max_seconds)
    import numpy
    import sherpa_onnx as sherpa

    segmentation = sherpa.OfflineSpeakerSegmentationModelConfig(
        pyannote=sherpa.OfflineSpeakerSegmentationPyannoteModelConfig(
            model=str(options.segmentation),
        ),
        num_threads=options.threads,
    )
    embedding = sherpa.SpeakerEmbeddingExtractorConfig(
        model=str(options.embedding), num_threads=options.threads,
    )
    clustering = sherpa.FastClusteringConfig(num_clusters=-1, threshold=options.threshold)
    config = sherpa.OfflineSpeakerDiarizationConfig(
        segmentation=segmentation, embedding=embedding, clustering=clustering,
        min_duration_on=0.3, min_duration_off=0.5,
    )
    if not config.validate():
        raise ValueError("The diarizer rejected the local model configuration.")
    diarizer = sherpa.OfflineSpeakerDiarization(config)
    samples, duration_ms = decode(options, data, diarizer.sample_rate, numpy)
    intervals = []
    for segment in diarizer.process(samples).sort_by_start_time():
        start = max(0, round(segment.start * 1000))
        end = min(duration_ms, round(segment.end * 1000))
        if end > start:
            intervals.append({"speaker": str(segment.speaker), "start_ms": start, "end_ms": end})
    model = (f"segmentation={model_digest(options.segmentation)};"
             f"embedding={model_digest(options.embedding)};threshold={options.threshold}")
    result = {"protocol_version": 1, "source_id": data["source_id"],
              "backend": f"sherpa-onnx {sherpa.__version__}", "model": model, "intervals": intervals}
    sys.stdout.buffer.write(json.dumps(result, ensure_ascii=True).encode("utf-8"))


if __name__ == "__main__":
    try:
        main()
    except (ImportError, OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(f"Local diarization failed: {error}")
