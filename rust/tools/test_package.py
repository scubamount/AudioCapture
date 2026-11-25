import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from check_package import verify_payload

MODULE = Path(__file__).with_name("package.py")
EXAMPLE_PATH = "config/transcription.example.json"
spec = importlib.util.spec_from_file_location("packager_under_test", MODULE)
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)


class PackagingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "project"
        (self.root / "src").mkdir(parents=True)
        (self.root / "src/main.rs").write_text("initial", encoding="utf-8")
        (self.root / "Cargo.toml").write_text("package", encoding="utf-8")
        (self.root / "Cargo.lock").write_text("locked", encoding="utf-8")
        (self.root / "README.md").write_text("readme", encoding="utf-8")
        self.adapter = self.root / "tools/diarize_local.py"
        self.adapter.parent.mkdir()
        self.adapter.write_text("# Optional local adapter", encoding="utf-8")
        self.example = self.root / EXAMPLE_PATH
        self.example.parent.mkdir()
        self.example_data = (MODULE.parent.parent / EXAMPLE_PATH).read_bytes()
        self.example.write_bytes(self.example_data)
        self.dep = self.root / "dependency"
        self.dep.mkdir()
        (self.dep / "LICENSE").write_text("license", encoding="utf-8")
        self.output = Path(self.temp.name) / "output.zip"
        self.built = []
        self.change_during_build = None
        self.additional_packages = []
        self.patches = [
            patch.object(packager, "ROOT", self.root),
            patch.object(packager, "cargo", self.cargo),
            patch.dict(os.environ, {}, clear=True),
        ]
        for item in self.patches:
            item.start()
            self.addCleanup(item.stop)

    def cargo(self, *args):
        command = args[0]
        if command == "metadata":
            return json.dumps({
                "resolve": {"root": "root-id"},
                "packages": [
                    {"id": "root-id", "name": "audio-capture", "version": "0.1.0"},
                    {"id": "dep-id", "name": "dep", "version": "1.0.0",
                     "manifest_path": str(self.dep / "Cargo.toml"), "license": "MIT"},
                    *self.additional_packages,
                ],
            })
        if command == "tree":
            return "audio-capture v0.1.0\ndep v1.0.0\n" + "".join(
                f"{package['name']} v{package['version']}\n"
                for package in self.additional_packages
            )
        if command != "build":
            raise AssertionError(args)
        self.built.append(args)
        self.assertIn("--offline", args)
        self.assertIn("--locked", args)
        self.assertIn("--release", args)
        self.assertIn("--bins", args)
        messages = []
        for name in packager.BINARIES:
            executable = self.root / f"{name}.exe"
            executable.write_text((self.root / "src/main.rs").read_text(), encoding="utf-8")
            messages.append(json.dumps({
                "reason": "compiler-artifact", "package_id": "root-id",
                "target": {"name": name, "kind": ["bin"]},
                "executable": str(executable),
            }))
        if self.change_during_build is not None:
            self.change_during_build.write_text("changed during build", encoding="utf-8")
        return "\n".join(messages)

    def run_main(self, output=None):
        with patch.object(sys, "argv", ["package.py", "--output", str(output or self.output)]):
            packager.main()

    def test_rebuilds_both_executables_and_archive_is_deterministic(self):
        (self.root / "audio-capture.exe").write_text("stale", encoding="utf-8")
        (self.root / "transcribe.exe").write_text("stale", encoding="utf-8")
        (self.root / "src/main.rs").write_text("updated", encoding="utf-8")
        for excluded in ("model.onnx", "python.exe", "private.opus", "test_package.py"):
            self.adapter.with_name(excluded).write_bytes(b"excluded")
        (self.root / "transcription.json").write_text("private settings", encoding="utf-8")
        self.example.with_name("personal.json").write_text("private settings", encoding="utf-8")
        self.run_main()
        with zipfile.ZipFile(self.output) as archive:
            self.assertEqual(archive.read("audio-capture.exe"), b"updated")
            self.assertEqual(archive.read("transcribe.exe"), b"updated")
            self.assertEqual(archive.read("tools/diarize_local.py"), self.adapter.read_bytes())
            self.assertEqual(archive.read(EXAMPLE_PATH), self.example_data)
            example = json.loads(archive.read(EXAMPLE_PATH))
            self.assertEqual(example["version"], 1)
            self.assertFalse(example["speech_gating"])
            self.assertFalse(example["speaker_labels"])
            self.assertEqual(set(archive.namelist()), {
                "audio-capture.exe", "transcribe.exe", "README.md", "tools/diarize_local.py",
                "licenses/DEPENDENCIES.md", "licenses/dep-1.0.0/LICENSE", EXAMPLE_PATH,
            })
        second = Path(self.temp.name) / "second.zip"
        self.run_main(second)
        self.assertEqual(self.output.read_bytes(), second.read_bytes())
        self.assertEqual(len(self.built), 2)

    def test_verify_payload_rejects_missing_example_and_unlisted_files(self):
        self.run_main()
        with zipfile.ZipFile(self.output) as archive:
            verify_payload(archive, self.root)
            entries = {name: archive.read(name) for name in archive.namelist()}
        mistakes = (
            {name: data for name, data in entries.items() if name != EXAMPLE_PATH},
            {**entries, "transcription.json": b"private"},
            {**entries, "licenses/dep-1.0.0/private.opus": b"private"},
            {**entries, EXAMPLE_PATH: b"{}"},
        )
        for index, contents in enumerate(mistakes):
            path = self.root / f"unexpected-{index}.zip"
            with zipfile.ZipFile(path, "w") as archive:
                for name, data in contents.items():
                    archive.writestr(name, data)
            with zipfile.ZipFile(path) as archive:
                with self.assertRaises(RuntimeError):
                    verify_payload(archive, self.root)

    def test_fallback_notice_requires_the_verified_sibling_text(self):
        bare = self.root / "bare-crate"
        bare.mkdir()
        self.additional_packages.append({
            "id": "bare-id", "name": "bare", "version": "1.0.0",
            "manifest_path": str(bare / "Cargo.toml"), "license": "MIT",
        })
        data = (self.dep / "LICENSE").read_bytes()
        upstream = "https://example.invalid/bare/LICENSE"
        fallback = {
            ("bare", "1.0.0"): ("dep", "1.0.0", "MIT", hashlib.sha256(data).hexdigest(), upstream),
        }
        with patch.object(packager, "LICENSE_FALLBACKS", fallback):
            self.run_main()
            with zipfile.ZipFile(self.output) as archive:
                verify_payload(archive, self.root)
                self.assertEqual(archive.read("licenses/bare-1.0.0/LICENSE-MIT.txt"), data)
                self.assertIn(upstream.encode(), archive.read("licenses/DEPENDENCIES.md"))
            (self.dep / "LICENSE").write_bytes(b"modified")
            second = self.root / "second.zip"
            with self.assertRaisesRegex(RuntimeError, "verified MIT text unavailable"):
                self.run_main(second)
            self.assertFalse(second.exists())

    def test_rejects_source_change_during_build(self):
        self.change_during_build = self.root / "src/main.rs"
        with self.assertRaisesRegex(RuntimeError, "changed while packaging"):
            self.run_main()
        self.assertFalse(self.output.exists())

    def test_rejects_adapter_change_during_build(self):
        self.change_during_build = self.adapter
        with self.assertRaisesRegex(RuntimeError, "changed while packaging"):
            self.run_main()
        self.assertFalse(self.output.exists())

    def test_rejects_example_change_during_build(self):
        self.change_during_build = self.example
        with self.assertRaisesRegex(RuntimeError, "changed while packaging"):
            self.run_main()
        self.assertFalse(self.output.exists())

    def test_missing_adapter_fails_before_build(self):
        self.adapter.unlink()
        with self.assertRaises(SystemExit):
            self.run_main()
        self.assertFalse(self.output.exists())
        self.assertEqual(self.built, [])

    def test_existing_output_is_not_replaced(self):
        self.output.write_bytes(b"previous")
        with self.assertRaises(SystemExit):
            self.run_main()
        self.assertEqual(self.output.read_bytes(), b"previous")
        self.assertEqual(self.built, [])

    def test_rejects_nonportable_override(self):
        with patch.dict(os.environ, {"RUSTFLAGS": "-C debuginfo=0"}):
            with self.assertRaisesRegex(RuntimeError, "static CRT"):
                self.run_main()
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
