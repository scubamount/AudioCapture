"""Create a Windows ZIP with a dependency inventory and license notices.

Run from the Rust project after building the desired release binary:
    python tools/package.py --output C:/path/to/audio-capture.zip

Cargo metadata and registry source packages must already be available offline.
The ZIP uses fixed timestamps, but the executable's bytes depend on the build.
"""

import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import sys
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
TARGET = "x86_64-pc-windows-msvc"
FIXED_TIME = (2020, 1, 1, 0, 0, 0)


def cargo(*args):
    return subprocess.check_output(
        ["cargo", *args], cwd=ROOT, text=True, encoding="utf-8"
    )


def archive_entry(archive, name, data):
    info = zipfile.ZipInfo(name, FIXED_TIME)
    info.compress_type = zipfile.ZIP_DEFLATED
    info.external_attr = 0o100644 << 16
    archive.writestr(info, data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    readme = ROOT / "README.md"
    if output.exists():
        parser.error(f"Output already exists: {output}")

    tree = cargo("tree", "--offline", "--locked", "--target", TARGET, "-e", "normal",
                 "--prefix", "none", "--format", "{p}")
    selected = set()
    for line in tree.splitlines():
        match = re.match(r"^([^ ]+) v([^ ]+)", line)
        if match:
            selected.add(match.groups())
    metadata = json.loads(cargo("metadata", "--offline", "--locked", "--format-version",
                                "1", "--filter-platform", TARGET))
    binary = pathlib.Path(metadata["target_directory"]) / TARGET / "release" / "audio-capture.exe"
    for source in (binary, readme):
        if not source.is_file():
            parser.error(f"Required file not found: {source}")
    packages = sorted(
        (p for p in metadata["packages"]
         if (p["name"], p["version"]) in selected and p["name"] != "audio-capture"),
        key=lambda p: (p["name"], p["version"]),
    )
    if not packages:
        raise RuntimeError("Cargo returned an empty runtime dependency set")
    found = {(p["name"], p["version"]) for p in packages}
    root_package = next(p for p in metadata["packages"] if p["id"] == metadata["resolve"]["root"])
    unmatched = selected - found - {(root_package["name"], root_package["version"])}
    if unmatched:
        raise RuntimeError(f"Missing Cargo metadata for: {sorted(unmatched)}")

    entries = {}
    index = ["# Runtime dependency inventory", "",
             "Cargo normal dependencies for " + TARGET + ".", "",
             "License expressions come from published crate metadata.", ""]
    missing = []
    for package in packages:
        name, version = package["name"], package["version"]
        directory = pathlib.Path(package["manifest_path"]).parent
        label = f"{name}-{version}"
        paths = [p for p in directory.iterdir() if p.is_file()
                 and p.name.lower().startswith(("license", "copying", "notice", "copyright"))]
        if package.get("license_file"):
            candidate = directory / package["license_file"]
            if candidate.is_file() and candidate not in paths:
                paths.append(candidate)
        extra = []
        if name == "opusic-sys":
            extra = [directory / "opus" / "COPYING"]
        elif name == "libflac-sys":
            extra = [directory / "flac" / "COPYING.Xiph"]
        paths += extra
        referenced = []
        for path in sorted(set(paths)):
            if not path.is_file():
                missing.append(f"{label}: missing {path.relative_to(directory)}")
                continue
            dest = f"licenses/{label}/{path.relative_to(directory).as_posix()}"
            entries[dest] = path.read_bytes()
            referenced.append(dest)
        if not referenced:
            missing.append(f"{label}: no license text found")
        index.append(f"- {label}: {package.get('license') or 'license not declared'}")
        index.extend(f"  - {path}" for path in referenced)
        if not package.get("license"):
            missing.append(f"{label}: no declared license expression")

    if missing:
        raise RuntimeError("Notice collection incomplete:\n" + "\n".join(missing))

    entries["audio-capture.exe"] = binary.read_bytes()
    entries["README.md"] = readme.read_bytes()
    entries["licenses/DEPENDENCIES.md"] = ("\n".join(index) + "\n").encode("utf-8")
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("xb") as destination, zipfile.ZipFile(destination, "w") as archive:
        for name, data in sorted(entries.items()):
            archive_entry(archive, name, data)
    print(f"Created {output}")
    print(f"Executable SHA-256: {hashlib.sha256(entries['audio-capture.exe']).hexdigest()}")
    print(f"Archive SHA-256: {hashlib.sha256(output.read_bytes()).hexdigest()}")
    print(f"Included {len(packages)} runtime dependency records and {len(entries) - 2} notices.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Packaging failed: {error}", file=sys.stderr)
        sys.exit(1)
