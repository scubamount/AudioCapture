"""Run an installed sherpa-onnx diarizer for one local source on request.

Install sherpa-onnx and NumPy in a separate Python environment. Install FFmpeg
and supply local ONNX segmentation and speaker embedding files. See the sherpa
speaker diarization guide for compatible models and their individual terms.
https://k2-fsa.github.io/sherpa/onnx/speaker-diarization/

Invoke this script through the worker's explicit adapter configuration.
The default mode reads one UTF-8 JSON request and exits after one source.
The live evaluation protocol accepts bounded PCM windows through local pipes.
"""

import argparse
from collections import Counter, defaultdict
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
    parser.add_argument("--live-windows", action="store_true",
                        help="Use the warm live-window-v1 PCM protocol")
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


def load_model(options, sherpa):
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
    return sherpa.OfflineSpeakerDiarization(config)


def offline(options, data, diarizer, numpy, sherpa):
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


RATE = 16000
CLOCK = 48000
CORE_SAMPLES = 4 * RATE
HISTORY_SAMPLES = 24 * RATE
MAX_TURNS = 512
MAX_SOURCES = 16


def anchor_votes(old, current, cutoff):
    """Compare only stretches with one active speaker in both analyses."""
    events = []
    for side, intervals in enumerate((old, current)):
        for start, end, label in intervals:
            if start < min(end, cutoff):
                events.append((start, side, label, 1))
                events.append((min(end, cutoff), side, label, -1))
    events.sort()
    active = [Counter(), Counter()]
    previous = 0
    votes = defaultdict(Counter)
    reverse = defaultdict(Counter)
    for at, side, label, delta in events:
        if at > previous and len(active[0]) == 1 and len(active[1]) == 1:
            stable, cluster = next(iter(active[0])), next(iter(active[1]))
            votes[cluster][stable] += at - previous
            reverse[stable][cluster] += at - previous
        active[side][label] += delta
        if active[side][label] == 0:
            del active[side][label]
        previous = at
    return votes, reverse


def supported(counts):
    if not counts:
        return None
    best, score = counts.most_common(1)[0]
    # Both directions must have a clear majority of shared, exclusive speech.
    if score >= CLOCK // 5 and score * 3 >= sum(counts.values()) * 2:
        return best
    return None


class LiveSource:
    def __init__(self):
        self.tail = None
        self.previous = []
        self.next_id = 1
        self.end = None

    def update(self, pcm, first, diarizer, numpy):
        if self.end is not None and first < self.end:
            raise ValueError("Speaker windows must not repeat committed audio.")
        if self.end != first:
            self.previous.clear()
            self.tail = None
        samples = pcm if self.tail is None else numpy.concatenate((self.tail, pcm))
        window_first = first - (len(samples) - len(pcm)) * 3
        self.end = first + len(pcm) * 3
        # Integer PCM travels through the pipe. Floating point conversion is
        # bounded by the lookback and core, regardless of the call duration.
        waveform = samples.astype(numpy.float32) / 32768.0
        decoded = diarizer.process(waveform).sort_by_start_time()
        if len(decoded) > MAX_TURNS:
            raise ValueError("Speaker window exceeded the turn limit.")
        current = []
        for s in decoded:
            start = max(window_first, window_first + round(s.start * CLOCK))
            end = min(self.end, window_first + round(s.end * CLOCK))
            if start < end:
                current.append((start, end, int(s.speaker)))
        votes, reverse = anchor_votes(self.previous, current, first)
        mapping = {}
        revised = []
        for start, end, cluster in current:
            if cluster not in mapping:
                candidate = supported(votes[cluster])
                if candidate is not None and supported(reverse[candidate]) == cluster:
                    mapping[cluster] = candidate
                else:
                    # Clusters split or merge as the model receives context.
                    # A new provisional ID establishes no historical identity.
                    mapping[cluster] = self.next_id
                    self.next_id += 1
            revised.append((start, end, mapping[cluster]))
        self.tail = samples[-HISTORY_SAMPLES:].copy()
        earliest = self.end - len(self.tail) * 3
        self.previous = [(max(start, earliest), end, label)
                         for start, end, label in revised if end > earliest]
        if len(self.previous) > MAX_TURNS * 4:
            raise ValueError("Speaker history exceeded the turn limit.")
        return window_first, revised


def emit(value):
    sys.stdout.buffer.write(json.dumps(value, separators=(",", ":")).encode("ascii") + b"\n")
    sys.stdout.buffer.flush()


def live(options, diarizer, numpy, sherpa):
    if diarizer.sample_rate != RATE:
        raise ValueError("The live protocol requires 16 kHz models.")
    emit({"protocol": "live-window-v1", "ready": True,
          "backend": f"sherpa-onnx {sherpa.__version__}",
          "threads": options.threads, "threshold": options.threshold,
          "segmentation_sha256": model_digest(options.segmentation),
          "embedding_sha256": model_digest(options.embedding)})
    sources = {}
    stream = sys.stdin.buffer
    while header := stream.readline(1025):
        if len(header) > 1024 or not header.endswith(b"\n"):
            raise ValueError("Invalid live window header.")
        message = json.loads(header)
        if message == {"reset": True}:
            sources.clear()
            emit({"reset": True})
            continue
        if (not isinstance(message, dict) or set(message) != {"source", "first_frame", "samples"}
                or any(type(v) is not int for v in message.values())):
            raise ValueError("Invalid live window fields.")
        source, first, count = (message[k] for k in ("source", "first_frame", "samples"))
        if not (0 <= source < MAX_SOURCES and 0 <= first <= 2**64 - 1 - count * 3
                and 0 < count <= CORE_SAMPLES):
            raise ValueError("Live window exceeds input limits.")
        raw = stream.read(count * 2)
        if len(raw) != count * 2:
            raise ValueError("Incomplete live PCM window.")
        if source not in sources:
            sources[source] = LiveSource()
        replace_from, turns = sources[source].update(numpy.frombuffer(raw, dtype="<i2"), first, diarizer, numpy)
        emit({"source": source, "first_frame": first, "end_frame": first + count * 3,
              "replace_from": replace_from,
              "turns": [{"start": start, "end": end, "speaker": str(label)}
                        for start, end, label in turns]})


def main():
    options = parse_options()
    data = None if options.live_windows else request(options.max_seconds)
    import numpy
    import sherpa_onnx as sherpa
    diarizer = load_model(options, sherpa)
    if options.live_windows:
        live(options, diarizer, numpy, sherpa)
    else:
        offline(options, data, diarizer, numpy, sherpa)


if __name__ == "__main__":
    try:
        main()
    except (ImportError, OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(f"Local diarization failed: {error}")
