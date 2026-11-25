# Audio Capture

Use the [maintainer wiki](docs/wiki/README.md) to find the source, OpenSpec change, and focused checks for your task.

| Change | Start here |
| --- | --- |
| Capture sources, Win32 layout, recording profiles, codecs | [Recording](docs/wiki/recording.md) |
| Offline MP4, model packs, CLI, MCP, consent | [Transcription](docs/wiki/transcription.md#offline-mp4) |
| Live text, endpoint batches, and recovery | [Transcription](docs/wiki/transcription.md#live-text-and-batch-scheduling) |
| Speaker attribution and memory | [Speaker workflow](docs/speaker-workflow.md) |
| Focused tests, package checks, and release evidence | [Verification](docs/wiki/verification.md) |

Read the linked source and relevant OpenSpec change before editing. Use tests and release receipts for claims about shipped behavior. Keep private media, captions, configs, and job state local.

The Rust app lives in [`rust/`](rust). Use the [operator guide](rust/README.md) for settings and the [build guide](BUILD.md) for packaging. The upstream C++ app has a separate [build guide](docs/legacy/BUILD.md).
