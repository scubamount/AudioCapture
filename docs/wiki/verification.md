# Verification

[Wiki home](README.md) · [Build prerequisites](../../BUILD.md) · [Measurements](../performance.md)

## Focused checks

Run these PowerShell commands from the repository's `rust` directory on Windows x64. Select the rows that cover the change.

| Concern | Command | Source |
| --- | --- | --- |
| Profile controls and layout | `cargo test --release --locked --bin audio-capture native_layout` | [Layout matrix and overlap regression](../../rust/src/native_layout.rs) |
| Controller and launch | `cargo test --release --locked --bin audio-capture native_controller`, `cargo test --release --locked --bin audio-capture worker_launch` | [Controller tests](../../rust/src/native_controller.rs), [launch and arguments](../../rust/src/worker_launch.rs) |
| Routing and capture timing | `cargo test --release --locked --lib sources::tests`, `cargo test --release --locked --lib capture::tests` | [Route tests](../../rust/src/sources.rs), [identity and clock tests](../../rust/src/capture.rs) |
| Mixing, source loss, and markers | `cargo test --release --locked --lib record::tests`, `cargo test --release --locked --lib markers::tests` | [Timeline and session tests](../../rust/src/record.rs), [marker sidecar tests](../../rust/src/markers.rs) |
| WAV and other codecs | `cargo test --release --locked --lib encode::tests`, `cargo test --release --locked --lib opus_writer::tests` | [Resampling and rollover](../../rust/src/encode.rs), [Opus decoding and interruption](../../rust/src/opus_writer.rs) |
| Profile behavior and setup compatibility | `cargo test --release --locked --lib prefs::tests` | [Profile matching, first run, and legacy imports](../../rust/src/prefs.rs) |
| Guided consent and job state | `cargo test --release --locked --bin transcribe guided::tests`, `cargo test --release --locked --bin transcribe jobs::tests` | [Prompt tests](../../rust/src/bin/transcribe/guided.rs), [cache and job tests](../../rust/src/bin/transcribe/jobs.rs) |
| HTTP and HTML review | `cargo test --release --locked --bin transcribe transport::tests`, `cargo test --release --locked --bin transcribe review::tests` | [Loopback mock tests](../../rust/src/bin/transcribe/transport.rs), [review rendering tests](../../rust/src/bin/transcribe/review.rs) |
| Long-file conversion | `cargo test --release --locked --bin transcribe chunks::tests` | [Chunk offsets, cleanup, and FFmpeg](../../rust/src/bin/transcribe/chunks.rs) |
| Package assembly | `python -B -m unittest discover -s tools -p test_package.py` | [Package tests](../../rust/tools/test_package.py) |

Put FFmpeg on `PATH` for the codec decode and chunk checks. The Opus chunk fixture needs FFmpeg with the `libopus` encoder. Some codec decode checks skip their external work without FFmpeg. [Codec checks](../../rust/src/encode.rs) · [Opus checks](../../rust/src/opus_writer.rs) · [Chunk checks](../../rust/src/bin/transcribe/chunks.rs)

## Live Windows checks

Use these ignored tests with a working playback endpoint and FFmpeg on `PATH`. The tests play generated tones. Run them from `rust`. [Recording integration tests](../../rust/tests/recording.rs)

```powershell
cargo test --release --locked --test recording selected_process_stays_isolated_across_sessions -- --ignored --nocapture
cargo test --release --locked --test recording an_exited_source_keeps_the_other_track_recording -- --ignored --nocapture
```

Use a physical microphone for microphone acceptance. Exercise the extracted package for picker, console, and sibling launch behavior. Record which endpoint and build each check used. [Recorder verification notes](../../openspec/changes/rewrite-windows-recorder/tasks.md) · [Integration verification notes](../../openspec/changes/integrate-recorder-and-transcription/tasks.md)

## Package and evidence

The [Windows workflow](../../.github/workflows/rust.yml) shows the hosted check selection and artifact configuration. The [packager](../../rust/tools/package.py) builds both executables from locked offline dependencies. It checks for source changes during the build and writes the guide and dependency notices into the ZIP. See the [build guide](../../BUILD.md) for exact package commands.

Loopback mocks measure request and batch behavior. They exclude recognition time and quality. The [reliability notes](../../openspec/changes/improve-recorder-reliability-and-efficiency/tasks.md) record synthetic interruption results and open hardware checks. The [transcription spike notes](../../openspec/changes/spike-on-demand-transcription/tasks.md) list remaining provider and recognition checks.
