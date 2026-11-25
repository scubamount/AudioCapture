"""Check the Windows release ZIP and validate its example with the packaged recorder."""

import argparse
from pathlib import Path
import re
import subprocess
import tempfile
from zipfile import ZipFile

from package import BINARIES, ROOT, SUPPORT_FILES


INVENTORY = "licenses/DEPENDENCIES.md"
EXAMPLE = "config/transcription.example.json"
NOTICE_PATH = re.compile(r"licenses/[\w.+-]+/(?:[\w.+-]+/)*[\w.+-]+")


def verify_payload(archive, source=ROOT):
    names = archive.namelist()
    if len({name.casefold() for name in names}) != len(names):
        raise RuntimeError("The release ZIP has duplicate entry names.")
    if INVENTORY not in names:
        raise RuntimeError("The release ZIP is missing the dependency inventory.")
    inventory = archive.read(INVENTORY).decode("utf-8")
    notices = [line[4:] for line in inventory.splitlines() if line.startswith("  - ")]
    if not notices or any(
        not NOTICE_PATH.fullmatch(name)
        or any(part.endswith(".") for part in name.split("/"))
        for name in notices
    ):
        raise RuntimeError("The dependency inventory has an invalid notice path.")
    expected = {f"{binary}.exe" for binary in BINARIES}
    expected.update(SUPPORT_FILES, notices, [INVENTORY])
    if set(names) != expected:
        raise RuntimeError("The release ZIP entries differ from the package allowlist.")
    for name in SUPPORT_FILES:
        if archive.read(name) != (source / name).read_bytes():
            raise RuntimeError(f"The packaged {name} differs from the source.")


def verify_example(extracted):
    with tempfile.TemporaryDirectory(prefix="audio-capture-config-cwd-") as directory:
        result = subprocess.run(
            [extracted / "audio-capture.exe", "--check-transcription-config", extracted / EXAMPLE],
            cwd=directory,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            encoding="utf-8",
            timeout=15,
            creationflags=subprocess.CREATE_NO_WINDOW,
        )
        if result.returncode or result.stdout.strip() != "Transcription config is valid.":
            raise RuntimeError(f"The packaged recorder rejected its example. {result.stderr.strip()}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    extracted = args.destination.resolve()
    with ZipFile(args.archive) as archive:
        verify_payload(archive)
        extracted.mkdir(parents=True)
        archive.extractall(extracted)
    verify_example(extracted)
    print("Public Windows ZIP and packaged example passed.")


if __name__ == "__main__":
    main()
