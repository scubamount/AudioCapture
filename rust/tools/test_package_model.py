import hashlib
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

import package_model as pack


class ModelPackageTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        payload = b"synthetic pack asset\n"
        self.name = "fixture.json"
        asset = pack.Asset(self.name, "https://example.invalid/asset", len(payload),
                           hashlib.sha256(payload).hexdigest())
        self.patch = patch.object(pack, "ASSETS", (asset,))
        self.patch.start()
        self.addCleanup(self.patch.stop)
        (self.root / self.name).write_bytes(payload)
        (self.root / "private.json").write_text("exclude")

    def test_builds_deterministic_allowlisted_offline_pack(self):
        first, second = self.root / "a.zip", self.root / "b.zip"
        with patch("urllib.request.urlopen", side_effect=AssertionError("network")):
            pack.build(self.root, first)
            pack.build(self.root, second)
        self.assertEqual(first.read_bytes(), second.read_bytes())
        with zipfile.ZipFile(first) as z:
            self.assertEqual(sorted(z.namelist()), sorted([self.name, "README.md", "SHA256SUMS.txt"]))
            self.assertIn(pack.ASSETS[0].sha256, z.read("SHA256SUMS.txt").decode())

    def test_corruption_fails_without_leaving_an_archive(self):
        src = self.root / self.name
        src.write_bytes(b"x" * src.stat().st_size)
        out = self.root / "bad.zip"
        with self.assertRaises(ValueError):
            pack.build(self.root, out)
        self.assertFalse(out.exists())

    def test_existing_output_survives(self):
        out = self.root / "existing.zip"
        out.write_bytes(b"existing")
        with self.assertRaises(FileExistsError):
            pack.build(self.root, out)
        self.assertEqual(out.read_bytes(), b"existing")
