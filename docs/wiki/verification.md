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
| Live wire and slow worker | `cargo test --release --locked --lib live_wire::` | [Framing, loss, and stop tests](../../rust/src/live_wire.rs) |
| WAV and other codecs | `cargo test --release --locked --lib encode::tests`, `cargo test --release --locked --lib opus_writer::tests` | [Resampling and rollover](../../rust/src/encode.rs), [Opus decoding and interruption](../../rust/src/opus_writer.rs) |
| Profile behavior and setup compatibility | `cargo test --release --locked --lib prefs::tests` | [Profile matching, first run, and legacy imports](../../rust/src/prefs.rs) |
| Recent program identity | `cargo test --release --locked --lib recent_programs::` | [Restart, ambiguity, invalidation, and persistence](../../rust/src/recent_programs.rs) |
| Offline MP4, pinned model, and agent control | `cargo test --release --locked --bin transcribe local_batch`, `cargo test --release --locked --bin transcribe model_pack`, `cargo test --release --locked --bin transcribe control::` | [Job service](../../rust/src/bin/transcribe/local_batch.rs), [pack checks](../../rust/src/bin/transcribe/model_pack.rs), [agents](../../rust/src/bin/transcribe/control.rs) |
| Guided endpoint consent and job state | `cargo test --release --locked --bin transcribe guided::tests`, `cargo test --release --locked --bin transcribe jobs::tests` | [Prompt tests](../../rust/src/bin/transcribe/guided.rs), [cache and job tests](../../rust/src/bin/transcribe/jobs.rs) |
| Live text and batch admission | `cargo test --release --locked --bin transcribe live::`, `cargo test --release --locked --bin transcribe batch_queue::` | [Live worker](../../rust/src/bin/transcribe/live.rs), [batch queue](../../rust/src/bin/transcribe/batch_queue.rs) |
| HTTP and HTML review | `cargo test --release --locked --bin transcribe transport::tests`, `cargo test --release --locked --bin transcribe review::tests` | [Loopback mock tests](../../rust/src/bin/transcribe/transport.rs), [review rendering tests](../../rust/src/bin/transcribe/review.rs) |
| Long-file conversion | `cargo test --release --locked --bin transcribe chunks::tests` | [Chunk offsets, cleanup, and FFmpeg](../../rust/src/bin/transcribe/chunks.rs) |
| Speaker attribution and memory | `cargo test --release --locked --bin transcribe attribution::` | [Local worker](../../rust/src/bin/transcribe/attribution) |
| App and model package assembly | `python -B -m unittest discover -s tools -p 'test_package*.py'` | [App package tests](../../rust/tools/test_package.py), [model package tests](../../rust/tools/test_package_model.py) |

Put FFmpeg on `PATH` for the codec decode and chunk checks. The Opus chunk fixture needs FFmpeg with the `libopus` encoder. Some codec decode checks skip their external work without FFmpeg. [Codec checks](../../rust/src/encode.rs) · [Opus checks](../../rust/src/opus_writer.rs) · [Chunk checks](../../rust/src/bin/transcribe/chunks.rs)

## Live Windows checks

Use these ignored tests with a working playback endpoint and FFmpeg on `PATH`. The tests play generated tones. Run them from `rust`. [Recording integration tests](../../rust/tests/recording.rs)

```powershell
cargo test --release --locked --test recording selected_process_stays_isolated_across_sessions -- --ignored --nocapture
cargo test --release --locked --test recording an_exited_source_keeps_the_other_track_recording -- --ignored --nocapture
```

Use a physical microphone for microphone acceptance. Exercise the extracted package for picker, console, and sibling launch behavior. Record which endpoint and build each check used. [Recorder verification notes](../../openspec/changes/rewrite-windows-recorder/tasks.md) · [Integration verification notes](../../openspec/changes/integrate-recorder-and-transcription/tasks.md)

## Live transcription checks

Build both release executables before you test the recorder. Select a program with generated speech and, when available, a test microphone. Use a local endpoint. Check that **Stop recording** keeps text active and **Stop transcription** keeps audio active. Start a new interval while the prior output finalizes. Confirm **Stop both** uses one cutoff.

Use the extracted worker and a loopback mock to check committed text, independent source IDs, bounded stop, batch admission, and interruption recovery. Use the mock to verify request and cache behavior. Run recognition and diarization qualification with the pinned models and reference calls. [Acceptance plan](../../openspec/changes/add-independent-live-transcription/acceptance.md)

From `rust`, run `python -B tools/test_live_worker.py PATH\transcribe.exe` with the extracted worker. The check uses generated audio and an isolated local endpoint. [Loopback worker tests](../../rust/tools/test_live_worker.py)

Run the Windows playback integration on a working playback endpoint. The test plays two generated process tones and uses a local mock endpoint. Set the worker path to the extracted executable. Set `AUDIO_CAPTURE_LIVE_TEST_OUT` if you need a specific evidence folder. [Playback integration](../../rust/tests/live_session.rs)

```powershell
$env:AUDIO_CAPTURE_LIVE_TEST_WORKER = 'C:\path\to\extracted\transcribe.exe'
cargo test --release --locked --test live_session live_session_real_windows_acceptance -- --ignored --nocapture
```

Check the bitrate and sample rate labels at each target width and DPI. Test focus and horizontal overlap while both activities run. Confirm a nearby status announces waiting, recording, and completed output without announcing every word. [Native UI requirements](../../openspec/changes/add-independent-live-transcription/specs/live-recorder-interface/spec.md)

Record the hardware, fixture identity, endpoint timing, and build for resource measurements. Keep capture and worker results tied to the measured workload. [Live performance procedures](../benchmarks/live-performance.md)

## Package and evidence

The app packager builds both executables from locked offline dependencies. It checks for source changes during the build and includes the guide, example config, and dependency notices. The release workflow builds the optional pinned model ZIP separately. [App packager](../../rust/tools/package.py) · [Model packager](../../rust/tools/package_model.py) · [Build guide](../../BUILD.md)

CI checks the extracted worker's help, version, local CLI, and MCP discovery. A synthetic two-track test checks consent, cancellation, speaker review, and memory export and replay. It supplies generated audio and plain transcript text. It checks that the worker leaves speakers unidentified. [Windows workflow](../../.github/workflows/rust.yml) · [Package smoke check](../../rust/tools/check_release.py)

To repeat the worker check, run `python -B rust/tools/check_release.py <path-to-extracted-transcribe.exe>` from the repository root. Compare downloads with the named rows in `SHA256SUMS.txt` from the same release. [Download verification](../../README.md#download-for-windows-11-x64)

Loopback mocks measure request and batch behavior. They exclude recognition time and quality. The [reliability notes](../../openspec/changes/improve-recorder-reliability-and-efficiency/tasks.md) record synthetic interruption results and open hardware checks. The [transcription spike notes](../../openspec/changes/spike-on-demand-transcription/tasks.md) list remaining provider and recognition checks.

## Check offline MP4 on Windows

Extract the app ZIP and the verified English pack into separate folders. Use a local English MP4 that you can inspect. Run from `rust`.

```powershell
python -B tools/test_local_mp4.py C:\Test\App\audio-capture.exe C:\Test\clip.mp4 C:\Test\Model --output "$env:TEMP\audio-capture-mp4-check"
```

The harness exercises the UI, CLI, and MCP with an inactive desktop and no playback. It compares output bytes, segment timing, and the source hash. Keep the report and transcripts outside the repository. [Offline harness](../../rust/tools/test_local_mp4.py) · [Measured sample](../performance.md#offline-mp4-transcription)

## Check the speaker workflow

Use generated speech or a local recording you can inspect. Record the adapter version, model files, and source hashes. Check text coverage, speaker intervals, inferred names, and overlapping speech. Preserve private test inputs and exact transcript excerpts outside the repository.

Preview the export before you approve it. Import a batch twice to check replay behavior. Apply a revision and then import its correction. Remove the source from the approved scope and check the retraction. [Operator workflow](../speaker-workflow.md) · [Export contract](../../openspec/changes/add-speaker-attribution-and-memory-export/specs/transcript-memory-export/spec.md)

The fixture script generates audio from the installed English David, Mark, and Zira OneCore voices. It uses Python with NumPy and pywin32. The scoring stages also use psutil. Keep all generated files outside the repository. [Fixture script](../../rust/tools/test_speaker_acceptance.py)

From `rust`, set `$adapterConfig` to a JSON file containing the local adapter object. Set `$speakerWorker` to a built `transcribe.exe`. Use new output directories for each run.

```powershell
$corpus = Join-Path $env:TEMP 'audio-capture-speaker-corpus'
python .\tools\test_speaker_acceptance.py generate $corpus
python .\tools\test_speaker_acceptance.py acoustic $corpus --adapter $adapterConfig --results "$corpus-mix"
python .\tools\test_speaker_acceptance.py worker $corpus --adapter $adapterConfig --worker $speakerWorker --results "$corpus-worker"
```

Report fixture-based checks and recognition estimates with their input scope. A successful import or a short recorded call does not measure diarization accuracy across conversations. [Speaker acceptance criteria](../../openspec/changes/add-speaker-attribution-and-memory-export/design.md#planned-acceptance) · [Verification status](../../openspec/changes/add-speaker-attribution-and-memory-export/tasks.md)
