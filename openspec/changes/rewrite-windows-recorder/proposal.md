# Rust recorder for Windows

## Why

The Opus quality control disappears behind the active-audio filter in the reported recording workflow. The user needs legible settings and isolated application and microphone audio for transcription.

## What Changes

- Build an independent Rust recorder for the fork. Use Windows audio APIs and a lightweight, keyboard-accessible desktop UI.
- Give format, bitrate, sample rate, gain, and recording controls separate layout space.
- Preserve selected-process capture, microphone input, monitoring, recording modes, codecs, and reusable presets.
- Produce local audio files suitable for transcription. Treat built-in recognition and transcript analysis as separate product decisions.

## Capabilities

### New Capabilities

- `recorder-interface` covers responsive layout, units, keyboard access, and recording state.
- `audio-routing` covers process isolation, device selection, and monitoring.
- `recording-output` covers formats, mixing, synchronization, file integrity, and transcript input.
- `recorder-preferences` covers preferences and compatibility with saved presets.

### Modified Capabilities

The fork has no baseline OpenSpec capabilities.

## Impact

The proposed Windows x64 Rust executable can run alongside the existing recorder. Use a distinct executable and settings location during parity testing. The proposal introduces no runtime dependencies into the current C++ application.
