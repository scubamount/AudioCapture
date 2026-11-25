# Build Audio Capture

## Rust on Windows 11 x64

You need a stable x64 MSVC Rust toolchain, Visual Studio with **Desktop development with C++**, and a Windows SDK. Put CMake on `PATH` so Cargo can build the bundled codecs. Use CMake 3.21 or later with Visual Studio 2022. Install Python 3 to make the ZIP.

Run these commands from the repository root to build both executables.

```powershell
Set-Location rust
cargo build --release --locked --bins
```

By default, Cargo writes `audio-capture.exe` and `transcribe.exe` to `target\x86_64-pc-windows-msvc\release` under `rust`. Cargo reads the target and static runtime settings from `rust/.cargo/config.toml`.

For a long checkout path, set `$env:CARGO_TARGET_DIR = Join-Path $env:TEMP 'audio-capture-build'` before you build. If CMake chooses the wrong Visual Studio installation, set `$env:CMAKE_GENERATOR` to your installed generator, for example `Visual Studio 17 2022`.

## Make the Rust ZIP

Stay in `rust`. Fetch the locked dependencies while online. Run the offline packager.

```powershell
cargo fetch --locked
python .\tools\package.py --output .\AudioCapture-Rust-Windows-x64.zip
```

The packager builds both release executables with `--offline --locked`. It includes the operator guide, an example endpoint config, dependency notices, and the optional diarization bridge. Choose a new output path if the ZIP already exists.

Check the layout and smoke tests from a fresh extraction.

```powershell
$extracted = Join-Path $env:TEMP ('audio-capture-check-' + [guid]::NewGuid())
python -B tools/check_package.py .\AudioCapture-Rust-Windows-x64.zip $extracted
python -B tools/check_release.py (Join-Path $extracted 'transcribe.exe')
```

## Build the offline speech pack

The app ZIP and English model pack are separate downloads. Build the optional pack from the five pinned source files in an extracted official pack. The builder verifies their SHA-256 digests.

```powershell
python -B tools/package_model.py --source C:\Models\Whisper-base.en --output AudioCapture-Whisper-base.en-model.zip
```

Use `--download` in place of `--source C:\Models\Whisper-base.en` to fetch the pinned files online. The app imports the extracted pack and performs recognition on the CPU. [Model builder](rust/tools/package_model.py) · [Model import](rust/README.md#transcribe-an-mp4-offline)

The speaker bridge uses a separate Python environment, FFmpeg, and model files. Install them only when you want to run local diarization. [Speaker workflow](docs/speaker-workflow.md#optional-local-adapter)

## GitHub builds and releases

The Windows job checks formatting, charts, package assembly, and the Rust test suite. It builds and verifies the app ZIP. On a new release tag, it also builds the optional model ZIP. [Windows Rust build workflow](.github/workflows/rust.yml)

Push a new tag that matches the Cargo version, such as `v2.2.0`. Add release notes in `docs/releases/<tag>.md`. The publish job checks both ZIPs against `SHA256SUMS.txt` and confirms the tag points to the tested commit. Tags with a prerelease suffix publish as prereleases. [Releases](https://github.com/scubamount/AudioCapture/releases)

## Verify

Run the checks from `rust`.

```powershell
cargo test --release --locked --bin audio-capture native_layout
cargo test --release --locked --lib encode::tests
cargo test --release --locked --bin transcribe local_batch
cargo test --release --locked --bin transcribe control::
python -B -m unittest discover -s tools -p 'test_package*.py'
```

For a live process-isolation check, use a working Windows playback device. Put FFmpeg on `PATH`. The test plays generated tones. [Live test source](rust/tests/recording.rs)

```powershell
cargo test --release --locked --test recording selected_process_stays_isolated_across_sessions -- --ignored --nocapture
```

See the verification guide for the silent offline MP4 test and the endpoint smoke tests. [Verification](docs/wiki/verification.md)

## Original C++ build

Build the original recorder from the repository root with CMake and vcpkg. [C++ build guide](docs/legacy/BUILD.md)

To build it on GitHub, run **Legacy C++ build** from the Actions tab. The workflow stages the executable and guide as an Actions artifact. [Legacy workflow](.github/workflows/ci.yml)
