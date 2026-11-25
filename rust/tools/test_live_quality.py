"""Qualify local live speech processing against annotated and frozen audio.

This test tool uses the offline fixture generator, voices, and scoring rules
from test_speaker_acceptance. It is not part of the recorder package.
"""

import argparse
from collections import Counter
import json
import math
from pathlib import Path
from datetime import datetime, timezone
import subprocess
import threading
import time
import wave

import test_speaker_acceptance as offline


RATE = offline.RATE
TRACKS = {"mix": ("mix",), "stems": ("program", "mic")}
# The first and last cues are intentionally quiet. The other phrases include
# repeated words plus an interruption. Freeze these before comparing profiles.
SCRIPTS = (
    ("Open the wooden door.", "The small brass key is in the drawer.",
     "I will carry the empty boxes.", "Wait, wait, the light is blinking.",
     "Please count the blue chairs again.", "Close the window."),
    ("Check the green flag.", "The next train leaves the northern station.",
     "Put the paper tickets on the table.", "Yes, yes, that is the platform.",
     "We have time to buy some water.", "Watch the clock."),
    ("Read the first line.", "The spare folder is inside the cabinet.",
     "I found a copy of the letter.", "Look, look, the address has changed.",
     "Please send the new map tomorrow.", "Bring the stamp."),
    ("Move the orange cone.", "The last bicycle needs a new chain.",
     "I have a pump and a spare tire.", "Stop, stop, that wheel is loose.",
     "We can fix the brake this morning.", "Test the bell."),
)
CONFIRMATION_SCRIPTS = (
    ("Turn the small valve.", "The seed packets are in the green basket.",
     "I can water the plants after lunch.", "Here, here, the soil feels dry.",
     "Put the rake beside the wooden fence.", "Check the hose."),
    ("Lift the red cable.", "The second battery has a full charge.",
     "I see a warning on the display.", "Wait, wait, the fan has stopped.",
     "Give the motor another minute to cool.", "Read the meter."),
    ("Slice the brown bread.", "Three clean bowls are next to the sink.",
     "The onions need another minute.", "Look, look, the sauce is ready.",
     "Please leave a spoon beside each plate.", "Bring the salt."),
    ("Test the left lamp.", "The stage curtain should open after the bell.",
     "I will move the bench into position.", "Now, now, the actor is ready.",
     "Let the music play through the final scene.", "Dim the lights."),
)
LONG_GAP_SCRIPTS = (
    ("Fetch the thin brush.", "The paint on the first shelf is dry.",
     "I found a spare tin behind the blue box.", "Yes, yes, that shade will match.",
     "Leave room for the second coat.", "Please bring the ladder again.",
     "We should finish the top edge tonight.", "Cover the floor."),
    ("Count the spare screws.", "The middle hinge has come loose.",
     "A longer screw should reach the frame.", "Wait, wait, that one is bent.",
     "I can hold the door while you work.", "Please check the lower hinge.",
     "This time it closes without a sound.", "Test the latch."),
    ("Find the white mug.", "The coffee grinder is by the window.",
     "I measured enough beans for two cups.", "Look, look, the water is boiling.",
     "Give the grounds a moment to settle.", "There is cream in the small jug.",
     "We can take our cups to the porch.", "Leave the spoon."),
    ("Fold the soft cloth.", "The glass panel has a narrow crack.",
     "There is another pane in the back room.", "No, no, the wider one will fit.",
     "I can hold it steady while you measure.", "Place the ruler against the frame.",
     "We should put the tools away now.", "Sweep the steps."),
)
GATE_SCRIPTS = (
    ("Brush the gold frame.", "The old photograph shows a stone bridge.",
     "We should hang it beside the tall clock.", "Wait, wait, the hook is too low.",
     "I will get a longer piece of wire.", "Measure the wall."),
    ("Lift the gray lid.", "The blue crate holds two spare wheels.",
     "There is a brass pump under the bench.", "Yes, yes, the tube looks fine.",
     "We should check the pressure once more.", "Turn the wheel."),
    ("Move the short stool.", "The flour goes into the large bowl.",
     "I will pour the milk a little at a time.", "Stop, stop, the pan is hot.",
     "Let the batter rest near the stove.", "Set the timer."),
    ("Ring the brass bell.", "The ferry leaves after the next train.",
     "The last two tickets are in the envelope.", "Look, look, the boat is here.",
     "Please put our bags beside the rail.", "Hold the rope."),
)


def generate_gate(directory):
    """Use independent Flite voices for a single blind speech-gate comparison."""
    import numpy as np
    directory.mkdir(parents=True, exist_ok=False)
    voices = ("slt", "rms", "awb", "kal")
    cases = []
    for index, script in enumerate(GATE_SCRIPTS):
        dest = directory / f"call{index + 1:02}"
        dest.mkdir()
        stems = {track: np.zeros(48 * RATE) for track in TRACKS["stems"]}
        # Low-level stationary noise in one otherwise silent core must survive.
        stems["program"][24 * RATE:28 * RATE:31] = 1
        turns = []
        starts = [3970 - index * 20, 8720, 15570 + index * 50, 0, 32040, 43550 + index * 75]
        for i, text in enumerate(script):
            voice = voices[(index + (0, 1, 2, 0, 1, 2)[i]) % len(voices)]
            source = ("mic" if i in ((1, 3) if index % 2 else (0, 2, 5)) else "program")
            # Relative text files avoid platform-specific filter quoting.
            script_file = dest / f"line-{i+1}.txt"
            script_file.write_text(text, encoding="utf-8")
            synth = f"flite=voice={voice}:textfile={script_file.name}"
            pcm = subprocess.check_output(["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi",
                                          "-i", synth, "-ar", str(RATE), "-ac", "1", "-f", "s16le", "-"],
                                         cwd=dest)
            audio = np.frombuffer(pcm, dtype="<i2").astype(float)
            active = np.flatnonzero(abs(audio) > max(20, abs(audio).max() * .01))
            audio = audio[active[0]:active[-1] + 1]
            if i == 3:
                starts[i] = turns[-1]["end_ms"] - (340 + index * 100)
            quiet = i in (0, 5)
            first = starts[i] * RATE // 1000
            stems[source][first:first + len(audio)] += audio * (.035 if quiet else .55)
            turns.append({"start_ms": starts[i], "end_ms": round((first + len(audio)) * 1000 / RATE),
                          "text": text, "speaker": voice, "track": source, "quiet": quiet})
        for track, pcm in {**stems, "mix": stems["program"] + stems["mic"]}.items():
            offline.write_wav(dest / f"{track}.wav", pcm)
        cases.append({"case": dest.name, "duration_ms": 48000, "turns": turns,
                      "mix_sha256": offline.digest(dest / "mix.wav"),
                      "stems_sha256": {t: offline.digest(dest / f"{t}.wav") for t in stems}})
    manifest = directory / "corpus.json"
    offline.write(manifest, {"version": 1, "voices": list(voices), "cases": cases})
    print("Held-out gate manifest SHA-256", offline.digest(manifest))


def percentile(values, percent):
    if not values:
        return None
    return sorted(values)[max(0, math.ceil(len(values) * percent / 100) - 1)]


def generate(directory, confirmation, long_gap=False):
    import numpy as np

    directory.mkdir(parents=True, exist_ok=False)
    cache = directory / "sapi"
    cache.mkdir()
    cases = []
    scripts = LONG_GAP_SCRIPTS if long_gap else CONFIRMATION_SCRIPTS if confirmation else SCRIPTS
    for index, script in enumerate(scripts):
        dest = directory / f"held{index + 1:02}"
        dest.mkdir()
        voices = offline.VOICES
        cast = (1, 2, 0, 1, 0, 2) if confirmation else (0, 1, 2, 0, 1, 2)
        if long_gap:
            cast = (2, 0, 1, 2, 1, 0, 2, 1)
        roles = [voices[(index + value) % 3] for value in cast]
        mic = voices[(index + (1 if index % 2 else 2)) % 3]
        # Rotate the overlap between two remote voices and different stems.
        starts_ms = ([2250, 7350, 13400, 0, 21800, 33100] if confirmation
                     else [3950, 9000, 15500, 0, 24500, 35450])
        if long_gap:
            starts_ms = [3700, 6750, 11250, 0, 18750, 44800, 49750, 56450]
        clips, turns = [], []
        for i, (voice, words) in enumerate(zip(roles, script)):
            raw = offline.synthesize(voice, words, cache)
            if i == 3:
                starts_ms[i] = turns[-1]["end_ms"] - (800 if index % 2 else 600)
            start = starts_ms[i] * RATE // 1000
            track = "mic" if voice == mic else "program"
            quiet = i in (0, len(script) - 1)
            clips.append((track, start, raw * (.025 if quiet else .5)))
            turns.append({"start_ms": starts_ms[i], "end_ms": round((start + len(raw)) * 1000 / RATE),
                          "speaker": voice, "text": words, "track": track, "quiet": quiet})
        duration_ms = 60000 if long_gap else 40000
        length = duration_ms * RATE // 1000
        stems = {name: np.zeros(length) for name in TRACKS["stems"]}
        for name, start, audio in clips:
            stems[name][start:start + len(audio)] += audio
        for name, audio in {**stems, "mix": stems["program"] + stems["mic"]}.items():
            offline.write_wav(dest / f"{name}.wav", audio)
        cases.append({"case": dest.name, "duration_ms": duration_ms, "mix_sha256": offline.digest(dest / "mix.wav"),
                      "stems_sha256": {t: offline.digest(dest / f"{t}.wav") for t in stems},
                      "turns": turns})
    manifest = directory / "corpus.json"
    offline.write(manifest, {"version": 1, "voices": list(offline.VOICES), "cases": cases})
    print("Held-out corpus SHA-256", offline.digest(manifest))


def read_pcm(path):
    with wave.open(str(path)) as reader:
        if (reader.getframerate(), reader.getsampwidth(), reader.getnchannels()) != (RATE, 2, 1):
            raise ValueError("Expected 16 kHz mono PCM16")
        return reader.readframes(reader.getnframes())


class ResourceMonitor:
    """Sample the adapter plus active children, excluding the harness."""

    def __init__(self, pid):
        import psutil
        self.root = psutil.Process(pid)
        self.peak = 0
        self.private = 0
        self.closing = threading.Event()
        self.thread = threading.Thread(target=self.monitor, daemon=True)
        self.thread.start()

    def monitor(self):
        import psutil
        while not self.closing.is_set():
            try:
                family = [self.root, *self.root.children(recursive=True)]
                rss, private = 0, 0
                for child in family:
                    try:
                        info = child.memory_info()
                        rss += info.rss
                        private += info.private
                    except (psutil.NoSuchProcess, psutil.AccessDenied):
                        continue
                if private:
                    self.private = private
                    self.peak = max(self.peak, rss)
            except (psutil.NoSuchProcess, psutil.AccessDenied):
                break
            self.closing.wait(.03)

    def close(self):
        self.closing.set()
        self.thread.join()


class WarmAdapter:
    """Exercise the shipped window-mode bridge over its production protocol."""

    def __init__(self, args):
        cfg = offline.read(args.adapter)
        argv = [cfg["program"], "-I", "-B", str(args.bridge), "--live-windows", *cfg["args"][1:]]
        started = time.monotonic()
        self.proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.monitor = ResourceMonitor(self.proc.pid)
        try:
            message = self.receive()
            if message.get("ready") is not True or message.get("protocol") != "live-window-v1":
                raise ValueError(f"Unexpected adapter readiness: {message}")
        except BaseException:
            self.close()
            raise
        self.start_seconds = time.monotonic() - started

    def receive(self):
        line = self.proc.stdout.readline(1 << 20)
        if not line or not line.endswith(b"\n"):
            raise ValueError("Missing or oversized adapter result")
        return json.loads(line)

    def send(self, header, audio=b""):
        self.proc.stdin.write(json.dumps(header).encode("ascii") + b"\n")
        self.proc.stdin.write(audio)
        self.proc.stdin.flush()
        return self.receive()

    def window(self, source, frame, audio):
        response = self.send({"source": source, "first_frame": frame, "samples": len(audio) // 2}, audio)
        if response["source"] != source or response["first_frame"] != frame:
            raise ValueError("Mismatched source or frame in adapter reply")
        replace = response.get("replace_from", frame)
        if not 0 <= replace <= frame or response["end_frame"] != frame + len(audio) * 3 // 2:
            raise ValueError("Adapter reported invalid replacement range")
        if any(not replace <= t["start"] < t["end"] <= response["end_frame"] for t in response["turns"]):
            raise ValueError("Turn outside adapter replacement range")
        return replace, response["turns"]

    def close(self):
        if self.proc.stdin:
            self.proc.stdin.close()
        try:
            self.proc.wait(timeout=8)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
        finally:
            self.monitor.close()


def acoustic(args):
    cfg = offline.read(args.adapter)
    corpus = offline.read(args.corpus / "corpus.json")
    totals = Counter()
    rows = []
    peaks, cold, delays = [], [], []
    wall = 0.0
    for case in corpus["cases"]:
        start = time.monotonic()
        folder = args.corpus / case["case"]
        if any("track" not in turn for turn in case["turns"]):
            # The released fixture records the assignment in its fixed script.
            mic = offline.script(int(case["case"].removeprefix("case")) - 1)[3]
            for turn in case["turns"]:
                turn.setdefault("track", "mic" if turn["speaker"] == mic else "program")
        checksums = {"mix": case["mix_sha256"], **case.get("stems_sha256", {})}
        for name, expected in checksums.items():
            if offline.digest(folder / f"{name}.wav") != expected:
                raise ValueError(f"Audio differs from its manifest: {case['case']}/{name}")
        tracks = TRACKS[args.layout]
        spans = []
        if args.mode == "offline":
            for track in tracks:
                request = {"protocol_version": 1, "source_id": track,
                           "audio_path": str((folder / f"{track}.wav").resolve()),
                           "duration_ms": case["duration_ms"]}
                raw, stats = offline.monitored([cfg["program"], str(args.bridge), *cfg["args"][1:]], request)
                peaks.append(stats["peak_tree_rss_bytes_sampled"] / (1 << 20))
                for interval in json.loads(raw)["intervals"]:
                    spans.append({**interval, "speaker": track + "_" + str(interval["speaker"]), "track": track})
        else:
            adapter = WarmAdapter(args)
            cold.append(adapter.start_seconds)
            chunks = [read_pcm(folder / f"{track}.wav") for track in tracks]
            service_elapsed = 0.0
            delivery = []
            step = 4 * RATE * 2
            try:
                for offset in range(0, max(map(len, chunks)), step):
                    block_seconds = min(4.0, case["duration_ms"] / 1000 - offset / (RATE * 2))
                    available = offset / (RATE * 2) + block_seconds
                    service_elapsed = max(service_elapsed, available)
                    for src, (track, pcm) in enumerate(zip(tracks, chunks)):
                        part = pcm[offset:offset + step]
                        if not part:
                            continue
                        tick = time.monotonic()
                        replace, intervals = adapter.window(src, offset * 3 // 2, part)
                        service_elapsed += time.monotonic() - tick
                        # Later windows may revise earlier provisional labels
                        # for this source. Snapshots let us audit corrections.
                        replace_ms = replace / 48
                        spans = [span if span["track"] != track else
                                 {**span, "end_ms": min(span["end_ms"], replace_ms)}
                                 for span in spans if span["track"] != track or span["start_ms"] < replace_ms]
                        for t in intervals:
                            span = {"speaker": track + "_" + str(t["speaker"]), "start_ms": t["start"] / 48,
                                    "end_ms": t["end"] / 48, "track": track}
                            spans.append(span)
                        delivery.append((spans.copy(), service_elapsed))
            finally:
                adapter.close()
            peaks.append(adapter.monitor.peak / (1 << 20))
        counts, mapping = offline.score(case["turns"], spans, case["duration_ms"])
        totals.update(counts)
        extras = timeline_stats(case["turns"], spans, mapping, delivery if args.mode == "live" else None)
        delays.extend(extras.pop("delays_seconds"))
        wall += time.monotonic() - start
        source_counts = {}
        if args.layout == "stems":
            for track in tracks:
                selected = [turn for turn in case["turns"] if turn["track"] == track]
                track_spans = [span for span in spans if span["track"] == track]
                if selected:
                    source_counts[track] = dict(offline.score(selected, track_spans, case["duration_ms"])[0])
        rows.append({"case": case["case"], "counts": dict(counts), "source_counts": source_counts, **extras})
        print(case["case"], offline.metrics(counts), flush=True)
    duration = sum(c["duration_ms"] / 1000 for c in corpus["cases"])
    turn_count = sum(row["turn_count"] for row in rows)
    summary = {"mode": args.mode, "layout": args.layout, "corpus_sha256": offline.digest(args.corpus / "corpus.json"),
               "bridge_sha256": offline.digest(args.bridge), "audio_seconds": duration, "wall_seconds": wall,
               "warm_rtf": (wall - sum(cold)) / duration, "cold_seconds": cold,
               "peak_tree_mib": max(peaks), "metrics": offline.metrics(totals), "rows": rows,
               "eligible_assigned": len(delays), "eligible_unassigned": turn_count - len(delays) if args.mode == "live" else None,
               "assigned_late_over_15s": sum(d > 15 for d in delays), "label_delay_p95_s": percentile(delays, 95)}
    offline.write(args.output, summary)
    print(summary["metrics"])


def timeline_stats(turns, spans, mapping, delivery):
    """Assign one observed primary cluster per turn; count switches and missing speakers."""
    previous = {}
    switches = missing = 0
    delays = []
    for turn in turns:
        low, high = turn["start_ms"], turn["end_ms"]
        coverage = Counter()
        for span in spans:
            if span.get("track", "mix") not in ("mix", turn["track"]):
                continue
            dt = max(0, min(high, span["end_ms"]) - max(low, span["start_ms"]))
            if dt:
                coverage[span["speaker"]] += dt
        if not coverage:
            missing += 1
            continue
        cluster = coverage.most_common(1)[0][0]
        who = turn["speaker"]
        switches += int(who in previous and previous[who] != cluster)
        previous[who] = cluster
        if delivery is None:
            continue
        # Earliest correct half-coverage that survives every later revision.
        # Wrong labels and repeated overlapping hypotheses earn no time credit.
        since = None
        for snapshot, at in delivery:
            intervals = [(max(low, p["start_ms"]), min(high, p["end_ms"])) for p in snapshot
                         if mapping.get(p["speaker"]) == who and p["end_ms"] > low and p["start_ms"] < high]
            cursor = covered = 0
            for i, j in sorted(intervals):
                covered += max(0, j - max(i, cursor))
                cursor = max(cursor, j)
            if covered >= (high - low) / 2:
                if since is None:
                    since = at
            else:
                since = None
        if since is not None:
            delays.append(max(0, since - high / 1000))
    return {"turn_count": len(turns), "turns_without_activity": missing, "switches": switches,
            "delays_seconds": delays}


def soak(args):
    case = offline.read(args.corpus / "corpus.json")["cases"][0]
    pcm = [read_pcm(args.corpus / case["case"] / f"{name}.wav") for name in TRACKS["stems"]]
    step = 4 * RATE * 2
    chunks = [[audio[n:n + step] for n in range(0, len(audio), step)] for audio in pcm]
    adapter = WarmAdapter(args)
    private = {}
    first = 0
    counter = 0
    start = time.monotonic()
    utc_start = datetime.now(timezone.utc).isoformat()
    wall_marks = {}
    stop = threading.Event()

    def measure_wall():
        for mark in sorted({600, 1800, args.seconds}):
            if mark > args.seconds or stop.wait(max(0.0, start + mark - time.monotonic())):
                break
            wall_marks[str(mark)] = {"at_utc": datetime.now(timezone.utc).isoformat(),
                                     "elapsed": time.monotonic() - start,
                                     "private_mib": adapter.monitor.private / (1 << 20)}
            print("Wall sample", mark, wall_marks[str(mark)], flush=True)

    sampler = threading.Thread(target=measure_wall, daemon=True)
    sampler.start()
    try:
        while first < args.seconds * 48000:
            index = counter % len(chunks[0])
            if args.paced:
                available = (first + len(chunks[0][index]) * 3 // 2) / 48000
                time.sleep(max(0.0, start + available - time.monotonic()))
            for source, stream in enumerate(chunks):
                adapter.window(source, first, stream[index])
            first += len(chunks[0][index]) * 3 // 2
            counter += 1
            for mark in (600, 1800, args.seconds):
                if first >= mark * 48000 and str(mark) not in private:
                    private[str(mark)] = adapter.monitor.private / (1 << 20)
                    print("Processed audio seconds", mark, "private MiB", private[str(mark)], flush=True)
            if counter % 100 == 0:
                print("Processed", round(first / 48000), flush=True)
    finally:
        stop.set()
        sampler.join()
        adapter.close()
    offline.write(args.output, {"processed_call_seconds": first / 48000, "wall_seconds": time.monotonic() - start,
                                "paced": args.paced, "start_utc": utc_start, "wall_samples": wall_marks,
                                "warm_private_mib": private, "sample_period_s": .03,
                                "peak_tree_mib": adapter.monitor.peak / (1 << 20),
                                "bridge_sha256": offline.digest(args.bridge),
                                "corpus_sha256": offline.digest(args.corpus / "corpus.json")})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    gen = commands.add_parser("generate")
    gen.add_argument("directory", type=Path)
    variant = gen.add_mutually_exclusive_group()
    variant.add_argument("--confirmation", action="store_true", help="Generate the second held-out set")
    variant.add_argument("--long-gap", action="store_true", help="Generate the third held-out set")
    variant.add_argument("--gate-confirmation", action="store_true", help="Generate the independent Flite voice gate set")
    for command in ("acoustic", "soak"):
        sub = commands.add_parser(command)
        sub.add_argument("--corpus", type=Path, required=True)
        sub.add_argument("--adapter", type=Path, required=True, help="Existing offline adapter config with Python and model paths")
        sub.add_argument("--bridge", type=Path, required=True)
        sub.add_argument("--output", type=Path, required=True)
        if command == "acoustic":
            sub.add_argument("--mode", choices=("offline", "live"), required=True)
            sub.add_argument("--layout", choices=TRACKS.keys(), required=True)
        else:
            sub.add_argument("--seconds", type=int, default=7200)
            sub.add_argument("--paced", action="store_true", help="Present PCM at original real-time speed")
    args = parser.parse_args()
    if args.command == "generate":
        if args.gate_confirmation:
            generate_gate(args.directory.resolve())
        else:
            generate(args.directory.resolve(), args.confirmation, args.long_gap)
    elif args.command == "acoustic":
        acoustic(args)
    else:
        soak(args)


if __name__ == "__main__":
    main()
