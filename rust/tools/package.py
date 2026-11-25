"""Create a Windows ZIP with a dependency inventory and license notices.

Run from the Rust project:
    python tools/package.py --output C:/path/to/audio-capture.zip

This builds both release executables with Cargo's locked, offline dependency set.
Registry source packages must already be available offline. The ZIP includes the
operator guide and optional local diarization script. The ZIP uses fixed
timestamps, but the executables' bytes depend on the build.
"""

import argparse
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
TARGET = "x86_64-pc-windows-msvc"
FIXED_TIME = (2020, 1, 1, 0, 0, 0)
BINARIES = ("audio-capture", "transcribe")
SUPPORT_FILES = ("README.md", "tools/diarize_local.py")


def cargo(*args):
    return subprocess.check_output(
        ["cargo", *args], cwd=ROOT, text=True, encoding="utf-8"
    )


def archive_entry(archive, name, data):
    info = zipfile.ZipInfo(name, FIXED_TIME)
    info.compress_type = zipfile.ZIP_DEFLATED
    info.external_attr = 0o100644 << 16
    archive.writestr(info, data)


def source_digest():
    """Identify the local inputs that can change the package."""
    paths = [ROOT / "Cargo.toml", ROOT / "Cargo.lock"]
    paths.extend(ROOT / name for name in SUPPORT_FILES)
    for directory in (ROOT / "src", ROOT / ".cargo"):
        if directory.is_dir():
            paths.extend(path for path in directory.rglob("*") if path.is_file())
    build_script = ROOT / "build.rs"
    if build_script.exists():
        paths.append(build_script)
    digest = hashlib.sha256()
    for path in sorted(paths):
        name = path.relative_to(ROOT).as_posix().encode("utf-8")
        data = path.read_bytes()
        digest.update(len(name).to_bytes(8, "little"))
        digest.update(name)
        digest.update(len(data).to_bytes(8, "little"))
        digest.update(data)
    return digest.hexdigest()


def check_portable_flags():
    # Cargo environment flags replace the target configuration. A caller can
    # add remapping flags, but must retain the static runtime selection.
    override = next((os.environ[name] for name in (
        "CARGO_ENCODED_RUSTFLAGS", "RUSTFLAGS",
        "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS"
    ) if os.environ.get(name)), None)
    if override and ("+crt-static" not in override or "-crt-static" in override):
        raise RuntimeError("Rust flags override the portable static CRT configuration")
    c_flags = os.environ.get("_CL_", "")
    if c_flags and ("/MT" not in c_flags.upper() or "/MD" in c_flags.upper()):
        raise RuntimeError("_CL_ overrides the portable static CRT configuration")


def build_binaries(package_id):
    check_portable_flags()
    output = cargo("build", "--offline", "--locked", "--release", "--bins",
                   "--target", TARGET, "--message-format=json-render-diagnostics")
    binaries = {}
    for line in output.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError as error:
            raise RuntimeError("Cargo returned invalid build output") from error
        target = message.get("target", {})
        name = target.get("name")
        if (message.get("reason") == "compiler-artifact"
                and message.get("package_id") == package_id
                and "bin" in target.get("kind", [])
                and name in BINARIES and message.get("executable")):
            path = pathlib.Path(message["executable"])
            if path.name.lower() != f"{name}.exe" or not path.is_file():
                raise RuntimeError(f"Cargo returned an invalid executable for {name}")
            binaries[name] = path
    if set(binaries) != set(BINARIES):
        missing = sorted(set(BINARIES) - set(binaries))
        raise RuntimeError(f"Cargo did not build required binaries: {', '.join(missing)}")
    return binaries


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists():
        parser.error(f"Output already exists: {output}")
    for name in SUPPORT_FILES:
        if not (ROOT / name).is_file():
            parser.error(f"Required file not found: {name}")

    before = source_digest()
    metadata = json.loads(cargo("metadata", "--offline", "--locked", "--format-version",
                                "1", "--filter-platform", TARGET))
    root_package = next(p for p in metadata["packages"] if p["id"] == metadata["resolve"]["root"])
    binaries = build_binaries(root_package["id"])
    tree = cargo("tree", "--offline", "--locked", "--target", TARGET, "-e", "normal",
                 "--prefix", "none", "--format", "{p}")
    selected = set()
    for line in tree.splitlines():
        match = re.match(r"^([^ ]+) v([^ ]+)", line)
        if match:
            selected.add(match.groups())
    packages = sorted(
        (p for p in metadata["packages"]
         if (p["name"], p["version"]) in selected and p["name"] != "audio-capture"),
        key=lambda p: (p["name"], p["version"]),
    )
    if not packages:
        raise RuntimeError("Cargo returned an empty runtime dependency set")
    found = {(p["name"], p["version"]) for p in packages}
    unmatched = selected - found - {(root_package["name"], root_package["version"])}
    if unmatched:
        raise RuntimeError(f"Missing Cargo metadata for: {sorted(unmatched)}")

    entries = {}
    index = ["# Runtime dependency inventory", "",
             "Cargo normal dependencies for " + TARGET + ".", "",
             "License expressions come from published crate metadata.", "",
             "The optional diarization script uses a separate Python environment and local models.", ""]
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

    entries["licenses/DEPENDENCIES.md"] = ("\n".join(index) + "\n").encode("utf-8")
    notice_count = len(entries)
    for name, binary in binaries.items():
        entries[f"{name}.exe"] = binary.read_bytes()
    for name in SUPPORT_FILES:
        entries[name] = (ROOT / name).read_bytes()
    if source_digest() != before:
        raise RuntimeError("Build inputs changed while packaging; run the packager again")
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("xb") as destination, zipfile.ZipFile(destination, "w") as archive:
        for name, data in sorted(entries.items()):
            archive_entry(archive, name, data)
    print(f"Created {output}")
    for name in BINARIES:
        digest = hashlib.sha256(entries[f"{name}.exe"]).hexdigest()
        print(f"{name}.exe SHA-256: {digest}")
    print(f"Archive SHA-256: {hashlib.sha256(output.read_bytes()).hexdigest()}")
    print(f"Included {len(packages)} runtime dependency records and {notice_count} notices.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Packaging failed: {error}", file=sys.stderr)
        sys.exit(1)
