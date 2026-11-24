# Audio Capture for Windows

Record a program's output and a selected microphone on Windows 11 x64. The Rust executable uses native Windows controls and WASAPI. It writes audio locally. Speech recognition runs separately on the completed file.

## Record a conversation

1. Select the program in the process list. Use Refresh if you opened it after the recorder.
2. Select the microphone if you want your side of the conversation.
3. Select Opus, 32 kbps, and mono for a compact speech recording.
4. Select the combined file mode to save one file for transcription.
5. Choose the output folder. Start recording. Stop to finalize the file.

Use separate files to keep your microphone and each application on distinct tracks. Both mode saves the mix and all individual tracks. Whole-system capture includes every application on the default playback endpoint. Process capture includes children of the selected process. Several browser tabs can share one process tree.

## Size and quality

These sizes came from the Rust writers using the same 17.12-second synthetic speech input. Actual Opus sizes vary with content. WAV uses 16-bit PCM.

| Format | Channels | Setting | File size |
| --- | --- | --- | ---: |
| Opus | Stereo | 128 kbps | 263,059 bytes |
| Opus | Mono | 32 kbps | 65,625 bytes |
| Opus | Mono | 24 kbps | 50,176 bytes |
| WAV | Stereo | 48000 Hz | 3,287,312 bytes |
| WAV | Mono | 16000 Hz | 547,922 bytes |

Choose **WAV, 16000 Hz, mono** when the transcription tool expects PCM input. WAV also supports Auto, 44100, and 48000 Hz. Auto uses the engine's 48000 Hz rate. Choose **24 kbps mono Opus** for smaller spoken-word files. Listen to a sample from your microphone before using a lower bitrate for a long session.

Silence skipping shortens the timeline. Leave it off when transcript timestamps must match elapsed recording time. Combined mode saves space when a single final track is sufficient. FLAC retains the encoded PCM samples and compresses them without loss. Windows Media Foundation supplies MP3 encoding. On the tested PC, mono MP3 supports 128 kbps. Encoder delay and final padding affect MP3 duration.

## Presets

The Rust application stores its own settings under `%LOCALAPPDATA%\AudioCaptureRust`. It can read the original recorder's preferences and named presets. Select an available microphone when a stored endpoint cannot be resolved. Select a specific process when several running programs match a preset. The Rust version supports one microphone per session.

## Build

Use the x64 MSVC Rust toolchain, a Visual Studio C++ workload, Windows SDK, and CMake with support for your Visual Studio generator. From this directory:

```powershell
cargo build --release --locked
```

The binary is `target\x86_64-pc-windows-msvc\release\audio-capture.exe`. For long checkout paths, set `$env:CARGO_TARGET_DIR = Join-Path $env:TEMP 'audio-capture-build'` before building. If CMake selects an incomplete Visual Studio installation, set `$env:CMAKE_GENERATOR` to the installed generator, such as `Visual Studio 17 2022`.

The executable requires Windows Media Foundation. Windows N editions may need the Media Feature Pack. Cargo builds the bundled Opus and FLAC libraries. FFmpeg and ffprobe are optional test tools. To package the executable with dependency notices, run:

```powershell
python tools/package.py --output audio-capture-windows-x64.zip
```

## Verify

```powershell
cargo test --release --locked --bin audio-capture native_layout
cargo test --release --locked --lib encode::tests
cargo test --release --locked --test recording selected_process_stays_isolated_across_sessions -- --ignored --nocapture
```

The last test plays generated tones in two child processes and decodes their files with FFmpeg. It checks isolation across sessions, the mixed output, separate tracks, pause timing, and a terminated process. It does not use the microphone.

## Implementation and reference behavior

The reference fixture in `tests/fixtures` came from the upstream C++ WAV writer at commit `a7fdb4e`. The Rust regression parses each file and compares PCM bytes. Both writers produced the same samples. A small-limit regression also checks the Rust writer's frame-aligned rollover across multiple WAV files.

On one Windows 11 x64 host, the native Rust window used 3.33 MB of private memory during a ten-second hidden idle check. The original used 3.79 MB. Both consumed zero measurable CPU time during that interval. This measurement covers the tested idle state. A separate two-source mixer benchmark took 14.0 ms for 10,000 blocks after optimization, down from 136.5 ms in the initial Rust implementation.

Two consecutive live recordings captured different generated tones in isolation. A third recording saved a mixed track and both isolated tracks with matching durations. A local English speech recognizer recovered both synthetic phrases from a 16 kHz mono WAV and both compact Opus settings. Physical microphone capture, hot-unplug, and a clean-machine install still need separate checks.

The C++ recorder remains available at the repository root. This implementation uses Windows audio APIs and the fork's OpenSpec. Compare observable behavior with the original when porting a feature. Use decoded PCM samples, file metadata, settings values, and control bounds as comparison points. Compressed bytes and wall-clock filenames vary across runs. [OpenSpec change](../openspec/changes/rewrite-windows-recorder) · [Behavior-replay method](https://github.com/erkamyaman/rust-anything)
