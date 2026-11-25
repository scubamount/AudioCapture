# Audio Capture

![Audio Capture title beside two audio waveforms](docs/images/audio-capture-banner.webp)

Record selected Windows applications, system audio, and one microphone on Windows 11 x64. The Rust recorder uses native Windows controls and WASAPI. Save a mix, separate tracks, or both in WAV, MP3, Opus, or FLAC.

## Get the Rust build

Build the portable ZIP from source. [Build guide](BUILD.md)

If a successful **Windows Rust build** run is available, sign in to GitHub. Open its run page. Download **AudioCapture-Rust-Windows-x64** under **Artifacts**. Extract the download to get the portable ZIP. [GitHub Actions](https://github.com/scubamount/AudioCapture/actions)

Extract `AudioCapture-Rust-Windows-x64.zip`. Run `audio-capture.exe` from the extracted folder. The ZIP includes `transcribe.exe`, the operator guide as `README.md`, and dependency notices.

## Record and transcribe

1. Select one or more **Programs**, or select **Capture all system audio**.
2. Select a **Microphone** if you need one.
3. Choose a **Recording profile**.
4. Choose an **Output folder**.
5. Select **Start recording**.
6. Select **Stop** to finish the files.

[![Audio Capture recording profile menu showing six profiles and Custom](docs/images/recording-profiles.png)](docs/images/recording-profiles.png)

Select **Transcribe files…** to choose completed recordings. This opens the separate `transcribe.exe` console. It sends audio to your chosen endpoint after you confirm the job. You can also run the worker directly.

Use the worker's speaker commands to separate voices, review inferred names, and export complete transcripts for agent memory. The recorder loads no speaker model. Install the optional local adapter to run diarization on your computer. [Speaker workflow](docs/speaker-workflow.md)

## Documentation

- Recording profiles, settings, and transcription steps. [Operator guide](rust/README.md)
- Guides for recording, transcription, and troubleshooting. [Repository wiki](docs/wiki/README.md)
- Measurements and commands to repeat them. [Performance](docs/performance.md)
- OpenSpec changes. [Recording profiles](openspec/changes/add-efficient-recording-profiles) · [Rust recorder](openspec/changes/rewrite-windows-recorder) · [Transcription integration](openspec/changes/integrate-recorder-and-transcription)
- Speaker attribution and memory export requirements. [OpenSpec change](openspec/changes/add-speaker-attribution-and-memory-export)

## Original C++ recorder

The original Win32 executable is `AudioCapture.exe`. The upstream **Latest Build** prerelease comes from its C++ `master` branch. [Upstream C++ download](https://github.com/masonasons/AudioCapture/releases/tag/latest)

See the [C++ user guide](docs/legacy/README.md) and [C++ build guide](docs/legacy/BUILD.md).

## License

This project is provided as-is for educational and personal use.

## Credits

The original AudioCapture project is by masonasons. Its README states, “This was written with Claude Code.” [Original project](https://github.com/masonasons/AudioCapture)

Both recorders use WASAPI, Win32, Media Foundation, libopus, and libFLAC. The Rust recorder uses `rfd` for native file and folder selection. See the C++ guide and Rust package notices for additional dependencies.
