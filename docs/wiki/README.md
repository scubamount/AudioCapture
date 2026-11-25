# Audio Capture maintainer wiki

Audio Capture has two Windows x64 Rust executables. `audio-capture.exe` saves audio locally. The user selects completed files to send with `transcribe.exe`.

| Area | Start here |
| --- | --- |
| Sources, capture, file output, and presets | [Recording](recording.md) |
| Consent, uploads, retry, and local review | [Transcription](transcription.md) |
| Test commands, packaging, and evidence | [Verification](verification.md) |
| User actions and settings | [Rust operator guide](../../rust/README.md) |
| Toolchain and portable ZIP | [Build guide](../../BUILD.md) |
| Original Win32 recorder | [C++ operator guide](../legacy/README.md) and [C++ build guide](../legacy/BUILD.md) |

## Source and evidence

- Read the Rust source and tests for current behavior. Start at [the recorder entry point](../../rust/src/main.rs) or [the worker entry point](../../rust/src/bin/transcribe/main.rs).
- Use the relevant open OpenSpec change for intended behavior. Its `proposal.md`, `design.md`, and `specs/` describe the change. Its `tasks.md` lists work and can include verification notes.
- Use [performance measurements](../performance.md) and the task notes for results from specific runs. Check the workload and limits before you reuse a result.

| OpenSpec change | Scope |
| --- | --- |
| [Rewrite Windows recorder](../../openspec/changes/rewrite-windows-recorder) | Rust interface, audio routing, output, and preferences |
| [Add efficient recording profiles](../../openspec/changes/add-efficient-recording-profiles) | Encoding profiles, clean-start default, and compatibility |
| [Improve recorder reliability and efficiency](../../openspec/changes/improve-recorder-reliability-and-efficiency) | Source loss, timeline, markers, and encoder recovery |
| [Add transcription workflow](../../openspec/changes/add-transcription-workflow) | Standalone worker, job state, chunking, and review |
| [Integrate recorder and transcription](../../openspec/changes/integrate-recorder-and-transcription) | Native file selection, guided consent, and portable delivery |
| [Spike on demand transcription](../../openspec/changes/spike-on-demand-transcription) | Endpoint and recognition experiments with open follow-up work |
