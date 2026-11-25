import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

MODULE = Path(__file__).with_name("package.py")
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
        self.dep = self.root / "dependency"
        self.dep.mkdir()
        (self.dep / "LICENSE").write_text("license", encoding="utf-8")
        self.output = Path(self.temp.name) / "output.zip"
        self.built = []
        self.change_during_build = False
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
                ],
            })
        if command == "tree":
            return "audio-capture v0.1.0\ndep v1.0.0\n"
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
        if self.change_during_build:
            (self.root / "src/main.rs").write_text("changed during build", encoding="utf-8")
        return "\n".join(messages)

    def run_main(self, output=None):
        with patch.object(sys, "argv", ["package.py", "--output", str(output or self.output)]):
            packager.main()

    def test_rebuilds_both_executables_and_archive_is_deterministic(self):
        (self.root / "audio-capture.exe").write_text("stale", encoding="utf-8")
        (self.root / "transcribe.exe").write_text("stale", encoding="utf-8")
        (self.root / "src/main.rs").write_text("updated", encoding="utf-8")
        self.run_main()
        with zipfile.ZipFile(self.output) as archive:
            self.assertEqual(archive.read("audio-capture.exe"), b"updated")
            self.assertEqual(archive.read("transcribe.exe"), b"updated")
            self.assertIn("licenses/dep-1.0.0/LICENSE", archive.namelist())
        second = Path(self.temp.name) / "second.zip"
        self.run_main(second)
        self.assertEqual(self.output.read_bytes(), second.read_bytes())
        self.assertEqual(len(self.built), 2)

    def test_rejects_source_change_during_build(self):
        self.change_during_build = True
        with self.assertRaisesRegex(RuntimeError, "changed while packaging"):
            self.run_main()
        self.assertFalse(self.output.exists())

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
