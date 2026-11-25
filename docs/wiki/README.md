# Audio Capture maintainer wiki

Audio Capture has two Windows x64 Rust executables. `audio-capture.exe` selects sources, saves audio, and controls the two output intervals. `transcribe.exe` processes approved live audio or completed files. It also recovers live text and runs local speaker review.

| Area | Start here |
| --- | --- |
| Sources, recent programs, recording intervals, and saved audio | [Recording](recording.md) |
| Independent controls, partial text, recovery, and final transcript files | [Live transcription guide](../live-transcription.md) |
| Worker protocol, consent, batch scheduling, and speaker review | [Transcription](transcription.md) |
| Test commands, packaging, and evidence | [Verification](verification.md) |
| User actions and settings | [Rust operator guide](../../rust/README.md) |
| Toolchain and portable ZIP | [Build guide](../../BUILD.md) |
| Original Win32 recorder | [C++ operator guide](../legacy/README.md) and [C++ build guide](../legacy/BUILD.md) |

## Source and evidence

- Read the Rust source and tests for current behavior. Start at [the recorder entry point](../../rust/src/main.rs) or [the worker entry point](../../rust/src/bin/transcribe/main.rs).
- Use the relevant open OpenSpec change for intended behavior. Its `proposal.md`, `design.md`, and `specs/` describe the change. Its `tasks.md` lists work and can include verification notes.
- Use the task notes and measurement reports for results from specific runs. Check the workload and limits before you reuse a result. [Performance measurements](../performance.md) · [Live performance procedures](../benchmarks/live-performance.md) · [Speech qualification](../benchmarks/live-quality.md)

| OpenSpec change | Scope |
| --- | --- |
| [Rewrite Windows recorder](../../openspec/changes/rewrite-windows-recorder) | Rust interface, audio routing, output, and preferences |
| [Add efficient recording profiles](../../openspec/changes/add-efficient-recording-profiles) | Encoding profiles, clean-start default, and compatibility |
| [Improve recorder reliability and efficiency](../../openspec/changes/improve-recorder-reliability-and-efficiency) | Source loss, timeline, markers, and encoder recovery |
| [Add transcription workflow](../../openspec/changes/add-transcription-workflow) | Standalone worker, job state, chunking, and review |
| [Integrate recorder and transcription](../../openspec/changes/integrate-recorder-and-transcription) | Native file selection, guided consent, and portable delivery |
| [Spike on demand transcription](../../openspec/changes/spike-on-demand-transcription) | Endpoint and recognition experiments with open follow-up work |
| [Add speaker attribution and memory export](../../openspec/changes/add-speaker-attribution-and-memory-export) | On-demand speaker analysis, reviewed names, and revisioned memory batches |
| [Independent live transcription (proposal)](../../openspec/changes/add-independent-live-transcription/proposal.md) | Independent start and stop, recent programs, silence handling, live text, and optional speaker labels |
