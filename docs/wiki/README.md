# Audio Capture maintainer wiki

Audio Capture runs on Windows 11 x64. `audio-capture.exe` owns the window and WASAPI capture. Its `control` and `mcp` commands launch the worker without a console. `transcribe.exe` runs approved transcription, recovery, and speaker commands.

| Area | Start here |
| --- | --- |
| Sources, recent programs, recording intervals, and saved audio | [Recording](recording.md) |
| Offline English MP4, model import, protected jobs, and MCP | [Offline MP4](transcription.md#offline-mp4) |
| Independent intervals, endpoint requests, and recovery | [Live worker and endpoint queue](transcription.md#live-text-and-batch-scheduling) · [Operator steps](../live-transcription.md) |
| Speaker correction and approved memory export | [Speaker workflow](../speaker-workflow.md) |
| Test commands, packaging, and evidence | [Verification](verification.md) |
| User actions and settings | [Rust operator guide](../../rust/README.md) |
| Toolchain and portable ZIP | [Build guide](../../BUILD.md) |
| Original Win32 recorder | [C++ operator guide](../legacy/README.md) and [C++ build guide](../legacy/BUILD.md) |

## Feature scope in 2.2.1

| Workflow | Shipped behavior |
| --- | --- |
| Record | Select one or more programs, system audio, and one microphone. Save to WAV, Opus, MP3, or FLAC. [Recording](recording.md) |
| Local speech | Import the verified English pack. Transcribe one completed MP4 per approved job. Review timed text through the UI, CLI, or MCP. [Offline MP4](transcription.md#offline-mp4) |
| Endpoint speech | Choose a compatible endpoint for live capture or completed-file batches. Recording and live transcription have independent controls. [Endpoint jobs](transcription.md#transfer-and-local-state) |
| Speakers and memory | Review speaker labels and names through the separate optional adapter workflow. Export approved transcript entries with revision history. [Speaker workflow](../speaker-workflow.md) |

The OpenSpec roadmap includes native live recognition, native speaker processing, and agent control for capture. Track that work in the implementation checklist. [Native workflow tasks](../../openspec/changes/integrate-native-workflows-and-agent-control/tasks.md)

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
| [Native video audio ingest](../../openspec/changes/add-native-video-audio-ingest/proposal.md) | Windows decoding of the first embedded audio track in a submitted video |
| [Native workflows and agent control (proposal)](../../openspec/changes/integrate-native-workflows-and-agent-control/proposal.md) | Native UI, on-demand Rust speech models, and equivalent MCP and headless controls |
