"""Exercise local control, speaker, and memory commands in the Windows package."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import wave


def check_worker(worker, root):
    def run(*args, answers="", expected=0):
        result = subprocess.run(
            [worker, *args], cwd=root, input=answers, capture_output=True,
            encoding="utf-8", timeout=20, creationflags=subprocess.CREATE_NO_WINDOW,
        )
        if result.returncode != expected:
            raise RuntimeError(
                f"{args[0]} exited {result.returncode}, expected {expected}.\n"
                f"{result.stdout}\n{result.stderr}"
            )
        return result

    transcripts = {"program": "Program audio.", "microphone": "Microphone audio."}
    for index, (stem, text) in enumerate(transcripts.items(), start=1):
        with wave.open(str(root / f"{stem}.wav"), "wb") as audio:
            audio.setparams((1, 2, 16_000, 0, "NONE", "not compressed"))
            audio.writeframes(b"\0\0" * (1_600 * index))
        (root / f"{stem}.txt").write_text(text, encoding="utf-8")

    manifest = root / "session.json"
    prepare = (
        "prepare-session", "--session", manifest, "--guided", "--",
        "program.wav", "microphone.wav",
    )
    answers = "program.txt\nprogram\nmicrophone.txt\nmic\ny\n"
    assert "Cancelled" in run(*prepare, answers=answers + "no\n").stdout
    assert not manifest.exists(), "Cancelled preparation created a manifest"
    assert "Session manifest saved" in run(*prepare, answers=answers + "yes\n").stdout
    original = manifest.read_bytes()
    document = json.loads(original)
    assert document["shared_clock"] and len(document["sources"]) == 2
    assert not document["automatic_names"] and document["adapter"] is None
    assert "already exists" in run(*prepare, expected=1).stderr
    assert manifest.read_bytes() == original, "Preparation replaced an existing manifest"

    result = run("attribute", "--session", "missing.json", "--session", manifest, expected=1)
    assert "1 of 2" in result.stderr and "revision 1." in result.stdout
    review = json.loads(run("review-session", "--session", manifest, "--json").stdout)
    assert review["complete"] and not review["speakers"]
    assert {source["raw_text"] for source in review["sources"]} == set(transcripts.values())

    scope = root / "scope.json"
    scope.write_text(json.dumps({
        "schema_major": 1,
        "sessions": [{"manifest": "session.json", "revision": review["revision"]}],
    }), encoding="utf-8")
    stream = root / "stream"
    export = ("export-memory", "--scope", scope, "--output", stream)
    run(*export)
    assert not stream.exists(), "Preview wrote an export stream"
    run(*export, "--approve")
    batches = list(stream.rglob("manifest.json"))
    assert len(batches) == 1, f"Expected one approved batch, found {len(batches)}"

    consume = ("import-memory", "--batch", batches[0], "--output", root / "consumer")
    first = json.loads(run(*consume).stdout)
    replay = json.loads(run(*consume).stdout)
    assert first["action"] == "imported" and replay["action"] == "replay"
    assert first["stream_id"] == replay["stream_id"] and first["cursor"] == replay["cursor"]


def check_local_control(worker, root):
    env = os.environ.copy()
    env.pop("AUDIO_CAPTURE_GRANT_REF", None)
    env.update(AUDIO_CAPTURE_STATE_DIR=str(root / "control-state"), PATH="")

    def run(*args, messages=()):
        result = subprocess.run(
            args, cwd=root, env=env, capture_output=True, encoding="utf-8",
            input="".join(json.dumps(message) + "\n" for message in messages),
            timeout=20, creationflags=subprocess.CREATE_NO_WINDOW,
        )
        assert result.returncode == 0 and not result.stderr, (result.returncode, result.stderr)
        return [json.loads(line) for line in result.stdout.splitlines()]

    (capabilities,) = run(worker, "control", "diagnostics", "capabilities", "--json")
    assert capabilities["ok"] and capabilities["schema_version"] == 1
    assert capabilities["provider"] == "local" and capabilities["file_types"] == ["mp4"]
    assert capabilities["profile_id"] == "local-whisper"

    launcher = worker.with_name("audio-capture.exe").resolve(strict=True)
    negotiation, listing = run(launcher, "mcp", messages=(
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "release-check", "version": "1"},
        }},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
    ))
    assert (negotiation["jsonrpc"], negotiation["id"]) == ("2.0", 1)
    assert negotiation["result"]["protocolVersion"] == "2025-11-25"
    assert "tools" in negotiation["result"]["capabilities"]
    assert (listing["jsonrpc"], listing["id"]) == ("2.0", 2)
    tools = {tool["name"]: tool for tool in listing["result"]["tools"]}
    assert set(tools) == {
        "audio_capture_batch", "audio_capture_diagnostics",
        "audio_capture_profile", "audio_capture_transcript",
    }
    for tool in tools.values():
        assert tool["inputSchema"]["type"] == tool["outputSchema"]["type"] == "object"
        assert {"schema_version", "action"} <= set(tool["inputSchema"]["required"])
        assert {"schema_version", "ok"} <= set(tool["outputSchema"]["required"])
    assert set(tools["audio_capture_batch"]["inputSchema"]["properties"]["action"]["enum"]) == {
        "preview", "commit", "list", "status", "cancel",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("worker", type=Path, help="Path to the extracted transcribe.exe")
    worker = parser.parse_args().worker.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="audio-capture-release-") as directory:
        root = Path(directory)
        check_local_control(worker, root)
        check_worker(worker, root)
    print("Packaged local control, speaker, and memory commands passed.")


if __name__ == "__main__":
    main()
