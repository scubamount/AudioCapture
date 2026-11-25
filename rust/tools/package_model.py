"""Build the optional English model ZIP from the pinned upstream files."""

import argparse
import hashlib
import http.client
import os
import tempfile
import time
import urllib.error
import urllib.request
import zipfile
from dataclasses import dataclass
from pathlib import Path

REVISION = "911407f4214e0e1d82085af863093ec0b66f9cd6"
MODEL = "https://huggingface.co/openai/whisper-base.en"
FILES = f"{MODEL}/resolve/{REVISION}"
BLOCK = 1024 * 1024


@dataclass(frozen=True)
class Asset:
    name: str
    url: str
    size: int
    sha256: str


ASSETS = (
    Asset("config.json", f"{FILES}/config.json", 1937,
          "160c1df40a60d4ef5a6014d536fd845c33aabfd1f318458ac7f87cfe91ebbe76"),
    Asset("model.safetensors", f"{FILES}/model.safetensors", 290401888,
          "d4dd5542fd6a1d35639e21384238f3bfe6c557c849d392b5905d33ee29e71db5"),
    Asset("tokenizer.json", f"{FILES}/tokenizer.json", 2405679,
          "5eb60cec1e77aeeb6869a2bb5a8e01a84c3fe5d072d75369343021fe6f5310d0"),
    Asset("WHISPER-MODEL-CARD.md", f"{FILES}/README.md", 14846,
          "767c687e78cb62c9607ce6ff077ab3368dae8072cee8cab6aeaa1246fc6a021c"),
    Asset("LICENSE-APACHE-2.0.txt", "https://www.apache.org/licenses/LICENSE-2.0.txt",
          11358, "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30"),
)

GUIDE = f"""# English Whisper model for Audio Capture

Extract this ZIP to a folder. Open Audio Capture and select **Transcribe files…**.
Set **Model pack folder** to the extracted folder. Select **Import base.en pack**.
The app verifies the files before use. You can then transcribe a completed MP4 offline.

Model: OpenAI Whisper base.en (English), Safetensors.
License: Apache-2.0. See LICENSE-APACHE-2.0.txt and WHISPER-MODEL-CARD.md.
Source: {MODEL}/tree/{REVISION}
"""


def copy_verified(reader, writer, asset):
    digest = hashlib.sha256()
    size = 0
    while chunk := reader.read(BLOCK):
        size += len(chunk)
        if size > asset.size:
            raise ValueError(f"{asset.name} exceeds its pinned size")
        digest.update(chunk)
        if writer is not None:
            writer.write(chunk)
    if size != asset.size or digest.hexdigest() != asset.sha256:
        raise ValueError(f"Size or SHA-256 mismatch for {asset.name}")


def fetch(asset, directory):
    # Retry transport failures only. A digest mismatch stops publication.
    for attempt in range(3):
        try:
            request = urllib.request.Request(asset.url, headers={"User-Agent": "AudioCapture-model-pack"})
            with urllib.request.urlopen(request, timeout=90) as source:
                with (directory / asset.name).open("wb") as dest:
                    copy_verified(source, dest, asset)
            return
        except (urllib.error.URLError, TimeoutError, http.client.HTTPException, ConnectionError):
            if attempt == 2:
                raise
            time.sleep(2 ** attempt)


def member(name):
    info = zipfile.ZipInfo(name, (2020, 1, 1, 0, 0, 0))
    info.compress_type = zipfile.ZIP_DEFLATED
    info.create_system = 3
    info.external_attr = 0o100644 << 16
    return info


def build(source, output):
    if output.exists():
        raise FileExistsError(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output.parent) as scratch:
        tmp = Path(scratch)
        if source is None:
            source = tmp
            for asset in ASSETS:
                fetch(asset, source)
        archive = tmp / "pack.zip"
        sums = "".join(f"{a.sha256}  {a.name}\n" for a in ASSETS)
        with zipfile.ZipFile(archive, "x") as z:
            for asset in ASSETS:
                with (source / asset.name).open("rb") as reader, z.open(member(asset.name), "w") as writer:
                    copy_verified(reader, writer, asset)
            z.writestr(member("README.md"), GUIDE.encode("utf-8"))
            z.writestr(member("SHA256SUMS.txt"), sums.encode("ascii"))
        with zipfile.ZipFile(archive) as z:
            for asset in ASSETS:
                with z.open(asset.name) as reader:
                    copy_verified(reader, None, asset)
        if os.name == "nt":
            archive.rename(output)  # Windows refuses an existing destination.
        else:
            os.link(archive, output)  # Publish verified bytes without overwriting a competing build.


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--source", type=Path, help="Folder containing the five named upstream assets")
    mode.add_argument("--download", action="store_true", help="Fetch pinned upstream assets over HTTPS")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    build(args.source, args.output)
    print(f"Verified {args.output.name} ({args.output.stat().st_size:,} bytes)")


if __name__ == "__main__":
    main()
