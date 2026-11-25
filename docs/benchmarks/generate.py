"""Render the checked-in benchmark figures with Python's standard library."""

import argparse
import json
from math import ceil, floor, isfinite
from pathlib import Path
from statistics import median
from xml.sax.saxutils import escape


HERE = Path(__file__).resolve().parent
DEST = HERE.parent / "images" / "benchmarks"
WIDTH = 800
PAPER = "#faf9f5"
INK = "#172e35"
MUTED = "#465961"
GRID = "#b9c6c6"
TEAL = "#00685f"
PLUM = "#754382"
PALE = "#e2eeea"


class Figure:
    def __init__(self, title, description, height, *, data_file="data.json"):
        self.parts = [
            f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}"'
            f' viewBox="0 0 {WIDTH} {height}" role="img" aria-labelledby="title desc"'
            ' font-family="Arial, Helvetica, sans-serif">',
            f'<title id="title">{escape(title)}</title>',
            f'<desc id="desc">{escape(description)}</desc>',
            f'<metadata>Audio Capture benchmark. Aggregate data: ../../benchmarks/{data_file}. '
            'Methods and tables: ../../benchmarks/README.md.</metadata>',
            '<defs><pattern id="hatch" patternUnits="userSpaceOnUse" width="6" height="6">'
            '<rect width="6" height="6" fill="#edeaf0"/>'
            '<path d="M-1 1L1 -1M0 6L6 0M5 7L7 5" stroke="#765e80" '
            'stroke-width="1.4"/></pattern></defs>',
        ]
        self.rect(0, 0, WIDTH, height, PAPER)
        self.text(32, 48, title, size=30, bold=True)

    def rect(self, x, y, width, height, fill):
        self.parts.append(f'<rect x="{x:.2f}" y="{y:.2f}" width="{width:.2f}" '
                          f'height="{height:.2f}" fill="{fill}"/>')

    def line(self, x1, y1, x2, y2, *, color=GRID, weight=1, dashed=False):
        dash = ' stroke-dasharray="5 4"' if dashed else ""
        self.parts.append(f'<line x1="{x1:.2f}" y1="{y1:.2f}" x2="{x2:.2f}" y2="{y2:.2f}" '
                          f'stroke="{color}" stroke-width="{weight}"{dash}/>')

    def text(self, x, y, label, *, size=16, color=INK, bold=False, anchor="start"):
        weight = 700 if bold else 400
        self.parts.append(f'<text x="{x:.2f}" y="{y:.2f}" fill="{color}" font-size="{size}" '
                          f'font-weight="{weight}" text-anchor="{anchor}">{escape(str(label))}</text>')

    def marker(self, x, y, layout):
        if layout == "Separate tracks":
            self.parts.append(f'<circle cx="{x:.2f}" cy="{y:.2f}" r="6" fill="{TEAL}"/>')
        else:
            self.parts.append(f'<path d="M{x:.2f} {y-8:.2f}l8 8 -8 8 -8 -8z" fill="{PLUM}"/>')

    def finish(self):
        return "\n".join([*self.parts, "</svg>", ""])


def acoustic_metrics(row):
    der = sum(row[k] for k in ("miss_ms", "false_ms", "confusion_ms")) / row["ref_ms"]
    recall = row["recovered"] / row["isolated"]
    return der * 100, recall * 100


def speaker_attribution(data):
    models = data["acoustic"]
    goals = data["goals_percent"]
    corpus = data["corpus"]
    desc = ["Diarization error and isolated-turn recall for each model and source layout."]
    for model in models:
        for row in model["results"]:
            der, recall = acoustic_metrics(row)
            desc.append(f'{model["model"]}, {row["layout"]}: {der:.2f}% error, {recall:.2f}% recall.')
    fig = Figure("Speaker attribution by source layout", " ".join(desc), 558)
    fig.text(32, 80, f'{corpus["audio_cases"]} synthetic calls  ·  {corpus["voices"]} voices'
                     f'  ·  {corpus["seconds"]} seconds of audio', color=MUTED, size=18)
    for x, label in [(40, "Separate tracks"), (230, "Single mix")]:
        fig.marker(x, 113, label)
        fig.text(x + 15, 119, label)
    fig.line(385, 113, 416, 113, dashed=True, color=INK, weight=2)
    fig.text(425, 119, "Acceptance threshold")

    panels = [
        ("Diarization error", "lower is better", list(range(0, 31, 5)), goals["maximum_der"], "≤"),
        ("Isolated-turn recall", "higher is better", list(range(70, 101, 5)), goals["minimum_isolated_recall"], "≥"),
    ]
    left, right = 246, 754
    for index, (name, direction, ticks, goal, sign) in enumerate(panels):
        top = 160 + index * 193
        fig.text(32, top, name, size=21, bold=True)
        fig.text(right, top, f"Goal {sign} {goal}%   ·   {direction}", anchor="end", color=MUTED)

        def position(value):
            return left + (right - left) * (value - ticks[0]) / (ticks[-1] - ticks[0])

        target = position(goal)
        start, end = (left, target) if index == 0 else (target, right)
        fig.rect(start, top + 40, end - start, 111, PALE)
        for tick in ticks:
            x = position(tick)
            fig.line(x, top + 40, x, top + 151)
            fig.text(x, top + 31, f"{tick}%", size=14, color=MUTED, anchor="middle")
        fig.line(target, top + 40, target, top + 151, dashed=True, color=INK, weight=2)
        for j, model in enumerate(models):
            y = top + 76 + j * 55
            fig.text(32, y - 3, model["model"], bold=True)
            fig.text(32, y + 17, f'Clustering threshold {model["threshold"]:.2f}', size=14, color=MUTED)
            metric_values = [acoustic_metrics(row)[index] for row in model["results"]]
            fig.line(position(min(metric_values)), y, position(max(metric_values)), y, color=MUTED, weight=2)
            for row, value in zip(model["results"], metric_values):
                x = position(value)
                fig.marker(x, y, row["layout"])
                # Leave room for half a percentage label beside the threshold line.
                label_x = max(x, target + 36) if x >= target else min(x, target - 36)
                fig.text(label_x, y - 12, f"{value:.2f}%", size=15, bold=True, anchor="middle")
    fig.text(32, 539, "Separate tracks contain two program voices and one microphone voice.", color=MUTED)
    return fig.finish()


def automatic_naming(data):
    rows = data["naming"]
    desc = []
    for row in rows:
        intro, ctx = row["introductions"], row["contextual"]
        desc.append(f'{row["evidence"]}: {intro["correct"]} of {intro["eligible"]} introductions '
                    f'and {ctx["correct"]} of {ctx["eligible"]} contextual names recognized. '
                    f'{row["incorrect_named_timed_utterances"]} wrong out of '
                    f'{row["named_timed_utterances"]} automatically named turns.')
    fig = Figure("Automatic names by evidence source", " ".join(desc), 433)
    fig.text(32, 80, "Eligible naming opportunities in the synthetic calls", size=18, color=MUTED)
    fig.rect(32, 103, 22, 12, TEAL)
    fig.text(63, 115, "Correct automatic name")
    fig.rect(291, 103, 22, 12, "url(#hatch)")
    fig.text(322, 115, "Abstained")
    fig.text(32, 163, "Speaker evidence", bold=True)
    fig.text(240, 163, "Introductions", bold=True)
    fig.text(424, 163, "Contextual names", bold=True)
    fig.text(613, 163, "Wrong / named turns", bold=True)
    for i, row in enumerate(rows):
        top = 180 + 72 * i
        fig.line(32, top, 768, top)
        fig.text(32, top + 31, row["evidence"], bold=True)
        for field, x in [("introductions", 240), ("contextual", 424)]:
            result = row[field]
            correct = result["correct"]
            eligible = result["eligible"]
            width = 146 * correct / eligible
            fig.text(x, top + 25, f"{correct} / {eligible}", size=17, bold=True)
            fig.rect(x, top + 33, width, 10, TEAL)
            fig.rect(x + width, top + 33, 146 - width, 10, "url(#hatch)")
            fig.text(x, top + 62, f'{result["abstained"]} abstained', size=14, color=MUTED)
        total = row["named_timed_utterances"]
        fig.text(613, top + 27, f'{row["incorrect_named_timed_utterances"]} / {total}', size=18, bold=True)
        if total == 0:
            fig.text(613, top + 51, "No named turns", size=15, color=MUTED)
    fig.text(32, 414, "A suggestion held for review counts as an abstention.", color=MUTED)
    return fig.finish()


def opus_storage(data):
    storage = data["storage"]
    pairs = storage["pairs"]
    desc = " ".join(f'{p["kbps"]} kbps: regular Opus {p["regular_bytes"]:,} bytes, '
                    f'speech DTX {p["speech_dtx_bytes"]:,} bytes.' for p in pairs)
    fig = Figure("Storage for the same two speech tracks", desc, 436)
    fig.text(32, 80, f'{storage["tracks"]} simultaneous mono tracks  ·  '
                     f'{storage["seconds_per_track"]} seconds each', size=18, color=MUTED)
    left, right, axis_max = 198, 675, 160_000
    fig.text(left, 122, "Combined file size in kB  ·  1 kB = 1,000 bytes", bold=True)
    for tick in range(0, axis_max + 1, 40_000):
        x = left + (right - left) * tick / axis_max
        fig.line(x, 156, x, 372)
        fig.text(x, 147, tick // 1000, size=14, anchor="middle", color=MUTED)
    for i, pair in enumerate(pairs):
        y = 185 + i * 115
        fig.text(32, y, f'{pair["kbps"]} kbps', bold=True, size=20)
        for j, (key, label, color) in enumerate([("regular_bytes", "Regular Opus", TEAL),
                                                  ("speech_dtx_bytes", "Speech DTX", "url(#hatch)")]):
            y_bar = y + 20 + j * 33
            size = pair[key]
            width = (right - left) * size / axis_max
            fig.text(48, y_bar + 6, label)
            fig.rect(left, y_bar - 9, width, 18, color)
            fig.text(left + width + 9, y_bar + 6, f"{size:,} B", size=15, bold=True)
    fig.text(32, 410, "Speech DTX enables discontinuous transmission and selects the Opus voice mode.", color=MUTED)
    return fig.finish()


def silence_storage(data):
    storage = data["silence_storage"]
    modes = [("regular_bytes", "Regular Opus", TEAL),
             ("speech_dtx_bytes", "Speech DTX", "url(#hatch)")]
    desc = []
    for case in storage["cases"]:
        desc.append(case["label"] + ".")
        for row in case["results"]:
            skip = "on" if row["skip_silence"] else "off"
            seconds = row["saved_frames_per_track"] / storage["sample_rate_hz"]
            desc.append(f'Skip silence {skip} keeps {seconds:.2f} seconds per track. '
                        f'Regular Opus uses {row["regular_bytes"]:,} bytes. '
                        f'Speech DTX uses {row["speech_dtx_bytes"]:,} bytes.')
    fig = Figure("Skip silence and Opus DTX", " ".join(desc), 615)
    fig.text(32, 80, f'{storage["track_count"]} synchronized mono tracks  ·  '
                     f'{storage["kbps"]} kbps  ·  {storage["encoder_ref"]} encoder replay', size=18, color=MUTED)
    for x, (_, label, color) in zip([32, 206], modes):
        fig.rect(x, 105, 22, 12, color)
        fig.text(x + 31, 117, label)

    left, right, axis_max = 224, 674, 160_000
    fig.text(left, 151, "Combined file size in kB  ·  1 kB = 1,000 bytes", bold=True)
    for i, case in enumerate(storage["cases"]):
        top = 199 + i * 191
        fig.text(32, top, case["label"], size=20, bold=True)
        for tick in range(0, axis_max + 1, 40_000):
            x = left + (right - left) * tick / axis_max
            fig.line(x, top + 31, x, top + 158)
            fig.text(x, top + 24, tick // 1000, size=14, anchor="middle", color=MUTED)
        for j, row in enumerate(case["results"]):
            y = top + 48 + j * 61
            skip = "On" if row["skip_silence"] else "Off"
            seconds = row["saved_frames_per_track"] / storage["sample_rate_hz"]
            fig.text(32, y + 6, f"Skip silence {skip}", bold=True)
            fig.text(32, y + 28, f"{seconds:.2f} s per track", color=MUTED)
            for k, (key, _, color) in enumerate(modes):
                size = row[key]
                width = (right - left) * size / axis_max
                center = y + 23 * k
                fig.rect(left, center - 7, width, 14, color)
                fig.text(left + width + 9, center + 5, f"{size:,} B", size=15, bold=True)
    fig.text(32, 590, "Skip silence shortens playback only when all selected sources are quiet.", color=MUTED)
    return fig.finish()


def live_speaker_labels(data):
    trial = data["live_speaker_labels"]
    modes = ["Completed file", "Live prototype, 24 s history"]
    description = []
    for layout in trial["layouts"]:
        for mode, row in zip(modes, layout["results"]):
            der, recall = acoustic_metrics(row)
            description.append(f'{layout["label"]}, {mode}. '
                               f'{der:.2f}% diarization error. {recall:.2f}% isolated-turn recall.')
    fig = Figure("Speaker labels on held-out calls", " ".join(description), 496)
    fig.text(32, 80, f'{trial["calls"]} synthetic English calls  ·  {trial["audio_seconds"]:.0f} seconds',
             size=18, color=MUTED)
    colors = [TEAL, "url(#hatch)"]
    for x, label, color in zip([32, 206], modes, colors):
        fig.rect(x, 105, 22, 12, color)
        fig.text(x + 31, 117, label)

    panels = [("Diarization error", "Lower is better"), ("Isolated-turn recall", "Higher is better")]
    left, right = 207, 670
    for metric, (title, direction) in enumerate(panels):
        top = 155 + metric * 155
        fig.text(32, top, title, size=20, bold=True)
        fig.text(right, top, direction, color=MUTED, anchor="end")
        for tick in range(0, 101, 20):
            x = left + (right - left) * tick / 100
            fig.line(x, top + 33, x, top + 132)
            fig.text(x, top + 26, f"{tick}%", size=14, anchor="middle", color=MUTED)
        for j, layout in enumerate(trial["layouts"]):
            y = top + 55 + 48 * j
            fig.text(32, y + 13, layout["label"], bold=True)
            for k, (row, color) in enumerate(zip(layout["results"], colors)):
                value = acoustic_metrics(row)[metric]
                width = (right - left) * value / 100
                center = y + 18 * k
                fig.rect(left, center - 6, width, 12, color)
                fig.text(left + width + 9, center + 5, f"{value:.2f}%", size=15, bold=True)
    fig.text(32, 472, "Both modes used the same reference speaker times and scoring rules.", color=MUTED)
    return fig.finish()


def paired_recording_cost(measurements):
    baseline_version = measurements["baseline_tag"]
    comparison = f"size profile {baseline_version} vs size profile candidate"
    matched, = (row for row in measurements["recording_only"] if row["comparison"] == comparison)
    values = {}
    for run in matched["runs"]:
        pair, build = run["pair"], run["build"]
        cpu = run["cpu_percent_one_core"]
        peak = run["windows_peak_working_set_bytes"]
        builds = values.setdefault(pair, {})
        if build in builds or not isfinite(cpu) or cpu < 0 or peak <= 0:
            raise ValueError("Expected one finite CPU and peak-memory observation per build in each pair")
        builds[build] = cpu, peak
    if not values or sorted(values) != list(range(1, matched["pair_count"] + 1)):
        raise ValueError("Expected consecutive paired recording trials")
    deltas = []
    for pair, builds in sorted(values.items()):
        if builds.keys() != {"baseline", "candidate"}:
            raise ValueError(f"Pair {pair} must contain a baseline and candidate")
        base_cpu, base_peak = builds["baseline"]
        new_cpu, new_peak = builds["candidate"]
        deltas.append((pair, new_cpu - base_cpu, (new_peak - base_peak) / (1 << 20)))
    return baseline_version, matched["duration_per_trial_s"], deltas


def live_recording_cost(measurements):
    baseline_version, duration, deltas = paired_recording_cost(measurements)
    _, cpu_points, peak_mib = zip(*deltas)
    median_cpu = median(cpu_points)
    max_peak = max(peak_mib)
    cpu_limit, peak_limit = 1, 2
    summary = [f"Median paired CPU change {median_cpu:+.3f} percentage points. "
               f"CPU limit +{cpu_limit} point applies to the median. "
               f"Largest peak working-set change {max_peak:+.3f} MiB. "
               f"The +{peak_limit} MiB limit applies to every pair."]
    summary.extend(f"Pair {pair}: CPU {cpu:+.3f} points, peak working set {peak:+.3f} MiB."
                   for pair, cpu, peak in deltas)
    last_row = 207 + (len(deltas) - 1) * 32
    fig = Figure(f"Recording cost vs {baseline_version}", " ".join(summary), last_row + 108,
                 data_file="live-performance.json")
    fig.text(32, 79, f"{len(deltas)} paired trials  ·  {duration} seconds each  ·  Two generated playback processes",
             size=18, color=MUTED)
    fig.text(32, 106, "Matched size-optimized builds  ·  Window creation and painting excluded", color=MUTED)
    fig.text(32, 147, "Pair", bold=True)

    def draw_panel(left, right, title, unit, observations, limit=None):
        low = min(0, floor(min(observations)))
        high = max(1, ceil(max(observations)), limit or 0)
        spacing = max(1, ceil((high - low) / 4))
        low, high = floor(low / spacing) * spacing, ceil(high / spacing) * spacing

        def position(value):
            return left + (right - left) * (value - low) / (high - low)

        for tick in range(low, high + 1, spacing):
            x = position(tick)
            fig.line(x, 183, x, last_row + 14, color=INK if tick == 0 else GRID)
            fig.text(x, 175, f"{tick:+d}" if tick else "0", size=14, color=MUTED, anchor="middle")
        if limit is not None:
            x = position(limit)
            fig.line(x, 183, x, last_row + 14, dashed=True, color=INK, weight=2)
        fig.text(left, 147, f"{title} ({unit})", bold=True)
        for i, value in enumerate(observations):
            x, y = position(value), 207 + i * 32
            fig.rect(x - 4, y - 4, 8, 8, TEAL if limit is None else PLUM)
            fig.text(x + 12, y + 5, f"{value:+.2f}", size=15, bold=True)

    for i, (pair, _, _) in enumerate(deltas):
        fig.text(32, 212 + i * 32, pair)
    draw_panel(101, 390, "CPU", "percentage points", cpu_points)
    draw_panel(492, 729, "Peak working set", "MiB", peak_mib, peak_limit)
    fig.text(32, last_row + 51, f"Median CPU change {median_cpu:+.3f} pp", bold=True)
    fig.text(32, last_row + 77, f"+{cpu_limit} pp CPU limit applies to the median.", color=MUTED)
    fig.text(414, last_row + 51, f"Largest peak increase {max_peak:+.3f} MiB", bold=True)
    fig.text(414, last_row + 77, f"+{peak_limit} MiB dashed limit applies to each pair.", color=MUTED)
    return fig.finish()


def local_mp4(data):
    trial = data["local_mp4"]
    duration = trial["audio_seconds"]
    runs = trial["runs"]
    desc = [f"One English MP4 supplied {duration:.3f} seconds of decoded audio."]
    desc.extend(f'{run["interface"]} completed in {run["elapsed_seconds"]:.2f} seconds.'
                for run in runs)
    desc.append("Each interface ran once under varying machine load.")
    fig = Figure("Offline MP4 transcription time", " ".join(desc), 358)
    fig.text(32, 80, f'One English MP4  ·  {trial["model"]} on CPU  ·  One run per interface',
             size=18, color=MUTED)
    left, right = 186, 750
    axis_step = 60
    longest = max(duration, *(run["elapsed_seconds"] for run in runs))
    axis_max = axis_step * ceil((longest + 20) / axis_step)

    def position(seconds):
        return left + (right - left) * seconds / axis_max

    fig.text(left, 121, "Elapsed time in seconds", bold=True)
    for tick in range(0, axis_max + 1, axis_step):
        x = position(tick)
        fig.text(x, 150, tick, anchor="middle", size=14, color=MUTED)
        fig.line(x, 160, x, 299)
    for index, run in enumerate(runs):
        y = 182 + 47 * index
        end = position(run["elapsed_seconds"])
        fig.text(32, y + 6, run["interface"], bold=True)
        fig.rect(left, y - 10, end - left, 20, TEAL)
        fig.text(end + 10, y + 6, f'{run["elapsed_seconds"]:.2f} s', size=15, bold=True)
    reference = position(duration)
    fig.line(reference, 160, reference, 299, dashed=True, color=INK, weight=2)
    fig.text(reference, 121, f"{duration:.3f} s decoded audio", anchor="end", color=MUTED)
    fig.text(32, 332, "Machine load varied. All three runs produced identical text and timed JSON.",
             color=MUTED)
    return fig.finish()


def validate(data):
    mp4 = data["local_mp4"]
    runs = mp4["runs"]
    if len(runs) != 3 or len({run["interface"] for run in runs}) != 3:
        raise ValueError("Expected one observation for each of the three MP4 interfaces")
    durations = [mp4["audio_seconds"], *(run["elapsed_seconds"] for run in runs)]
    if not all(isfinite(seconds) and seconds > 0 for seconds in durations):
        raise ValueError("Expected positive finite MP4 durations")
    for model in data["acoustic"]:
        for row in model["results"]:
            if row["ref_ms"] <= 0 or min(row[k] for k in ("miss_ms", "false_ms", "confusion_ms")) < 0:
                raise ValueError(f'Invalid speaker durations for {model["model"]}')
            if not 0 <= row["recovered"] <= row["isolated"] or row["isolated"] == 0:
                raise ValueError(f'Invalid turn counts for {model["model"]}')
            der, recall = acoustic_metrics(row)
            if not (0 <= der <= 30 and 70 <= recall <= 100):
                raise ValueError("Extend the attribution plot axes to fit the new data")
    for row in data["naming"]:
        for key in ("introductions", "contextual"):
            counts = row[key]
            if (min(counts.values()) < 0 or counts["eligible"] <= 0
                    or counts["correct"] + counts["abstained"] != counts["eligible"]):
                raise ValueError(f'Check naming outcomes for {row["evidence"]}')
        if not 0 <= row["incorrect_named_timed_utterances"] <= row["named_timed_utterances"]:
            raise ValueError(f'Invalid named turn counts for {row["evidence"]}')
    for pair in data["storage"]["pairs"]:
        if not all(0 < pair[key] <= 160_000 for key in ("regular_bytes", "speech_dtx_bytes")):
            raise ValueError("Check file sizes and storage plot limits")
    silence = data["silence_storage"]
    if len(silence["cases"]) != 2 or silence["sample_rate_hz"] <= 0:
        raise ValueError("Expected two silence comparisons with a positive sample rate")
    for case in silence["cases"]:
        off, on = case["results"]
        if off["skip_silence"] is not False or on["skip_silence"] is not True:
            raise ValueError("Expected silence removal off and on in that order")
        if not 0 <= on["saved_frames_per_track"] <= off["saved_frames_per_track"]:
            raise ValueError("Silence removal cannot increase the number of saved frames")
        for row in (off, on):
            if not all(0 < row[k] <= 160_000 for k in ("regular_bytes", "speech_dtx_bytes")):
                raise ValueError("Check silence comparison file sizes and axis limits")
    trial = data["live_speaker_labels"]
    if len(trial["layouts"]) != 2 or trial["calls"] <= 0 or trial["audio_seconds"] <= 0:
        raise ValueError("Expected two source layouts and a non-empty corpus")
    for layout in trial["layouts"]:
        offline, live = layout["results"]
        if offline["ref_ms"] <= 0 or offline["isolated"] <= 0:
            raise ValueError("Expected scored reference durations and turns")
        if any(offline[key] != live[key] for key in ("ref_ms", "isolated")):
            raise ValueError("Offline and live measurements must use the same reference")
        for row in (offline, live):
            if min(row[k] for k in ("miss_ms", "false_ms", "confusion_ms")) < 0:
                raise ValueError("Expected non-negative diarization error durations")
            if not all(0 <= value <= 100 for value in acoustic_metrics(row)):
                raise ValueError("Check the acoustic counts and plot limits")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="compare figures without writing files")
    args = parser.parse_args()
    data = json.loads((HERE / "data.json").read_text(encoding="utf-8"))
    performance = json.loads((HERE / "live-performance.json").read_text(encoding="utf-8"))
    validate(data)
    stale = []
    figures = [("speaker-attribution", speaker_attribution(data)), ("automatic-naming", automatic_naming(data)),
               ("opus-storage", opus_storage(data)), ("live-silence-storage", silence_storage(data)),
               ("live-speaker-labels", live_speaker_labels(data)),
               ("live-recording-cost", live_recording_cost(performance)), ("local-mp4", local_mp4(data))]
    for name, svg in figures:
        path = DEST / f"{name}.svg"
        content = svg.encode("utf-8")
        if args.check:
            if not path.is_file() or path.read_bytes() != content:
                stale.append(path.name)
        else:
            DEST.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
    if stale:
        raise SystemExit("Regenerate the figures: " + ", ".join(stale))
    action = "Verified" if args.check else "Wrote"
    print(f"{action} {len(figures)} benchmark figures.")


if __name__ == "__main__":
    main()
