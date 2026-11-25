"""Generate and score an offline speech fixture. No recordings or models enter the repository.

Run generate on Windows with Python, numpy, pywin32, and installed English David,
Mark, and Zira OneCore voices. Scoring uses psutil only for resource sampling.
Each backend test uses the generated audio plus fixed reference recognition text
and segment times. This isolates diarization and naming from recognition errors.
Use a separate directory for each assessment. See --help for the three stages.
"""
import argparse
from collections import Counter
import hashlib
import itertools
import json
from pathlib import Path
import subprocess
import time
import wave

RATE = 16000
VOICES = ("DavidM", "MarkM", "ZiraM")
NAMES = ("Nia", "Mira", "Rosa", "Jade", "Nora", "Lena", "Iris", "Cora")
ACTIONS = (("send the agenda", "host the review"), ("share the notes", "check the schedule"),
           ("write the summary", "read the minutes"), ("prepare the list", "close the meeting"))


def read(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def write(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def script(index):
    """Freeze text, actors, overlap and levels before any model sees the audio."""
    a, b, c = [VOICES[(index + offset) % 3] for offset in range(3)]
    intro, contextual = NAMES[index % 8], NAMES[(index + 3) % 8]
    spec = {"intro": [a, intro], "context": [b, contextual]}
    first, second = ACTIONS[index % len(ACTIONS)]
    lines = []

    def say(actor, words, *, quiet=False, overlap=0):
        lines.append((actor, words, quiet, overlap))

    say(c, "Before we start, we need to check the meeting schedule.")
    say(a, ("My name is " if index % 2 else "Hello, my name is ") + intro + ".")
    say(c, 'The example says, "My name is Taylor."')
    say(a, f"{contextual}, can you {first}?")
    say(b, f"I will {first}.")
    say(c, "Robin sent a message about the date.")
    say(c, f"{contextual}, will you {second}?")
    say(b, f"I will {second}.")
    say(a, "Our next meeting begins at ten on Thursday morning.")
    say(c, "Thank you.", quiet=True, overlap=780 if index % 2 == 0 else 0)
    say(b, "I can bring the printed copies to that meeting.")
    roster = [{"record_id": "agenda", "revision": 1, "name": contextual,
               "text": f"{contextual} is responsible for the meeting."}]
    if index >= 20:
        # Four additional sessions test conflicting introductions, a single requester,
        # competing replies, and unavailable corroboration, respectively.
        mode = index - 20
        if mode == 0:
            say(a, "My name is Elise.")
            del spec["intro"]
        elif mode == 1:
            lines[6] = (a, lines[6][1], False, 0)
            del spec["context"]
        elif mode == 2:
            lines.insert(8, (a, f"I will {second}.", False, 850))
            del spec["context"]
        else:
            roster = []
            del spec["context"]
    return lines, roster, spec, c


def synthesize(voice, words, cache):
    import numpy as np
    from win32com.client import Dispatch

    key = hashlib.sha256((voice + "\0" + words).encode()).hexdigest()
    path = cache / (key + ".wav")
    if not path.exists():
        token = Dispatch("SAPI.SpObjectToken")
        token.SetId("HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Speech_OneCore"
                    "\\Voices\\Tokens\\MSTTS_V110_enUS_" + voice)
        engine = Dispatch("SAPI.SpVoice")
        engine.Voice = token
        stream = Dispatch("SAPI.SpFileStream")
        stream.Format.Type = 18  # SAFT16kHz16BitMono
        stream.Open(str(path), 3, False)
        try:
            engine.AudioOutputStream = stream
            engine.Speak(words)
        finally:
            stream.Close()
    with wave.open(str(path)) as wav:
        assert (wav.getframerate(), wav.getsampwidth(), wav.getnchannels()) == (RATE, 2, 1)
        samples = np.frombuffer(wav.readframes(wav.getnframes()), dtype="<i2").astype(float)
    active = np.flatnonzero(np.abs(samples) > max(20, np.abs(samples).max() * .01))
    if len(active) == 0:
        raise ValueError(f"Voice produced silence: {voice}")
    return samples[active[0]:active[-1] + 1]


def write_wav(path, samples):
    import numpy as np
    with wave.open(str(path), "wb") as wav:
        wav.setparams((1, 2, RATE, 0, "NONE", "not compressed"))
        wav.writeframes(np.clip(samples, -32767, 32767).astype("<i2").tobytes())


def artifact(turns, labels):
    segments = []
    for item in turns:
        segment = {"text": item["text"], "start": item["start_ms"] / 1000,
                   "end": item["end_ms"] / 1000}
        if labels:
            segment["speaker"] = item["speaker"]
        segments.append(segment)
    return {"text": " ".join(t["text"] for t in turns), "segments": segments}


def generate(root):
    import numpy as np
    root.mkdir(parents=True, exist_ok=True)
    if (root / "corpus.json").exists():
        raise ValueError("Select a fresh output directory to generate another corpus.")
    cache = root / "sapi"
    cache.mkdir(exist_ok=True)
    cases = []
    for index in range(24):
        label = f"case{index + 1:02d}"
        dest = root / label
        dest.mkdir()
        lines, roster, wanted, mic = script(index)
        end = 0
        clips, turns = [], []
        for actor, words, quiet, overlap in lines:
            clip = synthesize(actor, words, cache)
            start = end + (480 if not overlap else -overlap) * RATE // 1000
            clips.append((actor, start, clip * (.09 if quiet else .50)))
            end = max(end, start + len(clip))
            turns.append({"speaker": actor, "text": words,
                          "start_ms": round(1000 * start / RATE),
                          "end_ms": round(1000 * (start + len(clip)) / RATE)})
        duration = ((end + RATE // 2 + 15) // 16) * 16
        app, microphone = np.zeros(duration), np.zeros(duration)
        for actor, start, clip in clips:
            track = microphone if actor == mic else app
            track[start:start + len(clip)] += clip
        for name, data in (("mix", app + microphone), ("program", app), ("mic", microphone)):
            write_wav(dest / (name + ".wav"), data)
        (dest / "truth.rttm").write_text("".join(
            f"SPEAKER {label} 1 {t['start_ms']/1000:.3f} {(t['end_ms']-t['start_ms'])/1000:.3f} <NA> <NA> {t['speaker']} <NA> <NA>\n"
            for t in turns), encoding="utf-8")
        write(dest / "reference.json", artifact(turns, True))
        write(dest / "recognition.json", artifact(turns, False))
        for name, keep in (("program", lambda t: t["speaker"] != mic),
                           ("mic", lambda t: t["speaker"] == mic)):
            write(dest / (name + ".json"), artifact([t for t in turns if keep(t)], False))
        cases.append({"case": label, "duration_ms": duration // 16, "turns": turns,
                      "expected": wanted, "roster": roster, "negative_turns": [2, 5],
                      "mix_sha256": digest(dest / "mix.wav")})
    write(root / "corpus.json", {"version": 1, "voices": VOICES, "cases": cases})
    print("Frozen corpus", digest(root / "corpus.json"), flush=True)


def labelset(spans, point):
    return {s["speaker"] for s in spans if s["start_ms"] <= point < s["end_ms"]}


def score(ref, hyp, duration):
    refs, hyps = sorted({r["speaker"] for r in ref}), sorted({h["speaker"] for h in hyp})
    edges = {v for r in ref for v in (r["start_ms"], r["end_ms"])}
    # Standard 250 ms *total* no-score interval around each reference boundary.
    cuts = {0, duration, *edges, *(max(0, e - 125) for e in edges),
            *(min(duration, e + 125) for e in edges),
            *(v for h in hyp for v in (h["start_ms"], h["end_ms"]))}
    cuts = sorted(cuts)
    shared = Counter()
    regions = []
    for l, r in zip(cuts, cuts[1:]):
        mid = (l + r) / 2
        if any(abs(mid - boundary) < 125 for boundary in edges):
            continue
        active, predicted = labelset(ref, mid), labelset(hyp, mid)
        regions.append((r - l, active, predicted))
        for i, j in itertools.product(active, predicted):
            shared[i, j] += r - l
    # Three reference voices. Assigning dummy columns leaves unmatched voices anonymous.
    cols = hyps + [None] * max(0, len(refs) - len(hyps))
    selected = max(itertools.permutations(cols, len(refs)),
                   key=lambda group: sum(shared[p] for p in zip(refs, group)))
    mapping = {h: r for r, h in zip(refs, selected) if h is not None and shared[r, h]}
    result = Counter()
    for length, reference, prediction in regions:
        correct = len(reference.intersection(mapping.get(p) for p in prediction))
        miss = max(0, len(reference) - len(prediction))
        false = max(0, len(prediction) - len(reference))
        confusion = min(len(reference), len(prediction)) - correct
        for who in reference:
            result["voice_" + who + "_ref_ms"] += length
            if who in {mapping.get(p) for p in prediction}:
                result["voice_" + who + "_correct_ms"] += length
        for key, value in (("ref_ms", len(reference)), ("miss_ms", miss),
                           ("false_ms", false), ("confusion_ms", confusion)):
            result[key] += length * value
            if len(reference) >= 2:
                result["overlap_" + key] += length * value
    for t in ref:
        low, high = t["start_ms"], t["end_ms"]
        if high - low < 1000 or any(u is not t and min(high, u["end_ms"]) >
                                    max(low, u["start_ms"]) for u in ref):
            continue
        result["isolated"] += 1
        # Union intervals, even if an adapter emits adjacent or duplicate intervals.
        spans = sorted((max(low, h["start_ms"]), min(high, h["end_ms"])) for h in hyp
                       if mapping.get(h["speaker"]) == t["speaker"])
        last, covered = low, 0
        for first, end in spans:
            covered += max(0, end - max(last, first))
            last = max(last, end)
        recovered = covered * 2 >= high - low
        result["recovered"] += recovered
        result["voice_" + t["speaker"] + "_isolated"] += 1
        result["voice_" + t["speaker"] + "_recovered"] += recovered
    return result, mapping


def metrics(totals):
    def der(prefix):
        if not totals[prefix + "ref_ms"]:
            return None
        error = sum(totals[prefix + kind + "_ms"] for kind in ("miss", "false", "confusion"))
        return error / totals[prefix + "ref_ms"]
    return {"der": der(""), "overlap_der": der("overlap_"),
            "isolated_recall": totals["recovered"] / totals["isolated"] if totals["isolated"] else None}


def check_scorer():
    a = {"speaker": "one", "start_ms": 300, "end_ms": 2500}
    b = {"speaker": "two", "start_ms": 1200, "end_ms": 2100}
    swapped = [dict(a, speaker="B"), dict(b, speaker="A")]
    perfect, ids = score([a, b], swapped, 3000)
    assert metrics(perfect)["der"] == 0 and ids == {"B": "one", "A": "two"}
    partial, _ = score([a, b], swapped[:1], 3000)
    assert partial["miss_ms"] == 650 and metrics(partial)["overlap_der"] == .5
    wrong, _ = score([a], [dict(a, end_ms=2900)], 3000)
    assert wrong["false_ms"] == 275 and wrong["recovered"] == 1


def monitored(argv, stdin=None):
    import psutil
    import tempfile
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        start = time.perf_counter()
        p = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=out, stderr=err)
        if stdin is not None:
            p.stdin.write(json.dumps(stdin).encode())
        p.stdin.close()
        peak = 0
        proc = psutil.Process(p.pid)
        while p.poll() is None:
            try:
                tree = [proc, *proc.children(recursive=True)]
                peak = max(peak, sum(c.memory_info().rss for c in tree if c.is_running()))
            except psutil.NoSuchProcess:
                pass
            if time.perf_counter() - start > 180:
                p.kill()
                raise TimeoutError("Assessment subprocess exceeded 180 seconds")
            time.sleep(.03)
        out.seek(0)
        err.seek(0)
        if p.returncode:
            raise RuntimeError(err.read().decode(errors="replace") + out.read().decode(errors="replace"))
        return out.read(), {"elapsed_s": round(time.perf_counter() - start, 3),
                            "peak_tree_rss_bytes_sampled": peak}


def evaluate_audio(root, adapter, representation, output):
    output.mkdir(parents=True, exist_ok=False)
    cfg = read(adapter)
    rows, totals = [], Counter()
    for case in read(root / "corpus.json")["cases"]:
        path = root / case["case"] / "mix.wav"
        assert digest(path) == case["mix_sha256"]
        destination = output / case["case"]
        destination.mkdir()
        spans = []
        calls = []
        tracks = ["mix"] if representation == "mix" else ["program", "mic"]
        for track in tracks:
            request = {"protocol_version": 1, "source_id": track,
                       "audio_path": str(path.parent / (track + ".wav")),
                       "duration_ms": case["duration_ms"]}
            content, resource = monitored([cfg["program"], *cfg["args"]], request)
            data = json.loads(content)
            calls.append({"track": track, "backend": data["backend"], "model": data["model"],
                          **resource})
            write(destination / (track + "-adapter-result.json"), data)
            spans.extend(dict(segment, speaker=track + "_" + segment["speaker"])
                         for segment in data["intervals"])
        counts, ids = score(case["turns"], spans, case["duration_ms"])
        totals.update(counts)
        rows.append({"case": case["case"], "counts": counts, "metrics": metrics(counts),
                     "reference_mapping": ids, "sources": calls})
        print(case["case"], metrics(counts), flush=True)
    summary = {"corpus_sha256": digest(root / "corpus.json"),
               "counts": totals, "metrics": metrics(totals), "cases": rows,
               "adapter_file_sha256": {Path(a).name: digest(Path(a)) for a in cfg["args"] if Path(a).is_file()}}
    write(output / "acoustic-scores.json", summary)
    print("Pooled", metrics(totals), flush=True)
    return totals


def check_chunk_boundary(case, src, worker, dest):
    """Exercise worker aggregate ingestion on a real boundary in the fixture."""
    dest.mkdir()
    turns = case["turns"]
    cut = (turns[4]["end_ms"] + turns[5]["start_ms"]) // 2
    parts = []
    for start, end, group in ((0, cut, turns[:5]), (cut, case["duration_ms"], turns[5:])):
        shifted = [dict(t, start_ms=t["start_ms"] - start, end_ms=t["end_ms"] - start)
                   for t in group]
        response = artifact(shifted, True)
        parts.append({"offset_frames": start * 16, "frames": (end - start) * 16,
                      "sample_rate": RATE, "text": response["text"],
                      "segments": response["segments"], "speaker_segments": response["segments"]})
    aggregate = dest / "recognition.json"
    write(aggregate, {"source_sha256": case["mix_sha256"], "sample_rate": RATE, "parts": parts})
    manifest = dest / "manifest.json"
    write(manifest, {"schema_major": 1, "session_id": "chunk_check", "sources": [
        {"source_id": "mix", "role": "mix", "audio": str(src / "mix.wav"),
         "duration_ms": case["duration_ms"], "artifact": str(aggregate)}]})
    monitored([worker, "attribute", "--session", manifest])
    raw, _ = monitored([worker, "review-session", "--session", manifest, "--json"])
    doc = json.loads(raw)
    assert doc["complete"] and doc["sources"][0]["raw_text"] == "\n".join(t["text"] for t in parts)
    for expected in turns:
        assert any(t["start_ms"] == expected["start_ms"] and t["end_ms"] == expected["end_ms"]
                   and t["raw_text"].strip() == expected["text"] for t in doc["turns"]), expected


def run_worker(root, worker, adapter, output, representation, evidence):
    cfg = read(adapter)
    rows = []
    overall = {"reference": Counter(), "backend": Counter()}
    output.mkdir(parents=True, exist_ok=False)
    cases = read(root / "corpus.json")["cases"]
    check_chunk_boundary(cases[0], root / cases[0]["case"], worker, output / "chunks")
    for case in cases:
        src = root / case["case"]
        for mode, artifact_name in (("reference", "reference"), ("backend", "recognition")):
            if evidence not in ("both", mode):
                continue
            dest = output / (case["case"] + "-" + mode)
            dest.mkdir()
            sources = []
            for track in (["mix"] if representation == "mix" else ["program", "mic", "mix"]):
                location = src / (artifact_name + ".json")
                if track != "mix":
                    mic = script(int(case["case"][4:]) - 1)[3]
                    truths = [t for t in case["turns"] if (t["speaker"] == mic) == (track == "mic")]
                    location = dest / (track + ".json")
                    write(location, artifact(truths, mode == "reference"))
                wav = src / (track + ".wav")
                sources.append({"source_id": track, "role": track, "audio": str(wav),
                                "artifact": str(location), "duration_ms": case["duration_ms"],
                                "sha256": digest(wav)})
            manifest = {"schema_major": 1, "session_id": dest.name, "shared_clock": True,
                        "representation": "auto", "complete_stems": representation == "stems",
                        "sources": sources, "automatic_names": True,
                        "naming_context": {"roster": case["roster"]}}
            if mode == "backend":
                manifest["adapter"] = cfg
            path = dest / "session.json"
            write(path, manifest)
            _, resources = monitored([worker, "attribute", "--session", path])
            raw, _ = monitored([worker, "review-session", "--session", path, "--json"])
            doc = json.loads(raw)
            assert doc["complete"], (case["case"], doc["sources"])
            if representation == "stems":
                assert doc["alternates"] == ["mix"]
            for source in doc["sources"]:
                source_id = source["source_id"]
                if mode == "backend":
                    assert source["provenance"].get("diarization"), source["provenance"]
                turns = sorted((t for t in doc["turns"] if t["source_id"] == source_id),
                               key=lambda t: t["span_bytes"][0])
                location = next(s["artifact"] for s in sources if s["source_id"] == source_id)
                assert "".join(t["raw_text"] for t in turns) == read(location)["text"]
            hyp = [{"speaker": a["speaker_id"], "start_ms": a["start_ms"], "end_ms": a["end_ms"]}
                   for a in doc["activities"]]
            _, mapping = score(case["turns"], hyp, case["duration_ms"])
            expected = case["expected"]
            counters = Counter()
            inferred = set()
            for s in doc["speakers"]:
                if s.get("person_id"):
                    counters["automatic_confirmations"] += 1
                if s.get("name_status") != "inferred":
                    continue
                for proof in s["evidence"]:
                    if proof["status"] == "inferred":
                        inferred.add((proof["rule"], mapping.get(s["speaker_id"]), s["display_name"]))
                if [mapping.get(s["speaker_id"]), s["display_name"]] not in expected.values():
                    counters["wrong_names"] += 1
            # A globally correct cluster name can still label an individual
            # utterance incorrectly if the backend merged two voices.
            named = {s["speaker_id"]: s["display_name"] for s in doc["speakers"]
                     if s.get("name_status") == "inferred"}
            actual_names = dict(expected.values())
            for turn in doc["turns"]:
                speakers = turn["speaker_ids"]
                if len(speakers) != 1 or speakers[0] not in named:
                    continue
                truths = [t for t in case["turns"] if t["text"] == turn["raw_text"].strip()
                          and t["start_ms"] == turn["start_ms"] and t["end_ms"] == turn["end_ms"]]
                if len(truths) != 1:
                    continue
                counters["named_turns"] += 1
                if actual_names.get(truths[0]["speaker"]) != named[speakers[0]]:
                    counters["wrong_named_turns"] += 1
            for rule, pair in expected.items():
                counters[rule + "_eligible"] += 1
                source_rule = {"intro": "direct_introduction", "context": "corroborated_exchange"}[rule]
                counters[rule + "_correct"] += (source_rule, *pair) in inferred
            counters["negative_utterances"] += len(case["negative_turns"])
            if case["case"] in ("case21", "case22", "case23", "case24"):
                counters["negative_scenarios"] += 1
            overall[mode].update(counters)
            rows.append({"case": case["case"], "mode": mode, "counts": counters, **resources})
            print(case["case"], mode, dict(counters), flush=True)
    write(output / "naming-scores.json", {"corpus_sha256": digest(root / "corpus.json"),
          "worker_sha256": digest(Path(worker)), "counts": overall, "cases": rows})
    return overall


def main():
    check_scorer()
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("step", choices=("generate", "acoustic", "worker"))
    p.add_argument("corpus", type=Path, help="Local fixture directory")
    p.add_argument("--adapter", type=Path, help="JSON with absolute program and argument paths")
    p.add_argument("--evidence", choices=("both", "reference", "backend"), default="both",
                   help="Worker evaluation mode (reference labels or actual local adapter)")
    p.add_argument("--representation", choices=("mix", "stems"), default="mix")
    p.add_argument("--worker", type=Path, help="Built transcribe executable")
    p.add_argument("--results", type=Path, help="New directory for measurements and reports")
    args = p.parse_args()
    root = args.corpus.resolve()
    if args.step == "generate":
        generate(root)
    elif not args.adapter:
        p.error("Supply --adapter for assessment.")
    elif args.step == "acoustic":
        values = metrics(evaluate_audio(root, args.adapter, args.representation,
                                        args.results or root / (args.representation + "-scores")))
        if values["der"] > .20 or values["isolated_recall"] < .90:
            raise SystemExit("The held-out acoustic acceptance gate failed.")
    else:
        if not args.worker or not args.results:
            p.error("Supply --worker and --results for the worker assessment.")
        summary = run_worker(root, args.worker.resolve(), args.adapter, args.results.resolve(),
                             args.representation, args.evidence)
        ref = summary["reference"]
        if (ref["intro_correct"] < .90 * ref["intro_eligible"]
                or ref["context_correct"] < .80 * ref["context_eligible"]
                or any(v["wrong_names"] or v["wrong_named_turns"] or v["automatic_confirmations"]
                       for v in summary.values())):
            raise SystemExit("The held-out naming acceptance gate failed.")


if __name__ == "__main__":
    main()
