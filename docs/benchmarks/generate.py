"""Render the checked-in benchmark figures with Python's standard library."""

import argparse
import json
from pathlib import Path
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
    def __init__(self, title, description, height):
        self.parts = [
            f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}"'
            f' viewBox="0 0 {WIDTH} {height}" role="img" aria-labelledby="title desc"'
            ' font-family="Arial, Helvetica, sans-serif">',
            f'<title id="title">{escape(title)}</title>',
            f'<desc id="desc">{escape(description)}</desc>',
            '<metadata>Audio Capture benchmark. Aggregate data: ../../benchmarks/data.json. '
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


def validate(data):
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="compare figures without writing files")
    args = parser.parse_args()
    data = json.loads((HERE / "data.json").read_text(encoding="utf-8"))
    validate(data)
    stale = []
    for name, render in [("speaker-attribution", speaker_attribution), ("automatic-naming", automatic_naming),
                         ("opus-storage", opus_storage)]:
        path = DEST / f"{name}.svg"
        content = render(data).encode("utf-8")
        if args.check:
            if not path.is_file() or path.read_bytes() != content:
                stale.append(path.name)
        else:
            DEST.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
    if stale:
        raise SystemExit("Regenerate the figures: " + ", ".join(stale))
    print("Verified 3 benchmark figures." if args.check else "Wrote 3 benchmark figures.")


if __name__ == "__main__":
    main()
