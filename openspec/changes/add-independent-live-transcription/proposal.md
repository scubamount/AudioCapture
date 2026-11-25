## Why

A caller can already select Discord and a microphone and later transcribe the finished files. Today they cannot see a transcript during the call or stop saving audio while keeping the transcript. Selecting a familiar program also requires finding its current process or loading a saved setup.

## What Changes

- Provide independent **Start recording** and **Start transcription** actions. Share one capture stream per selected source. Support either action alone and either stop order.
- Offer recent programs and an optional exact-match selection on return. Opening a program alone starts no activity.
- Show live text from bounded audio chunks. Save confirmed text locally as it arrives. Publish a readable text file when that transcription interval ends.
- Offer optional local speaker labels, an offline refinement pass on available audio, and a completed-file batch queue.
- Keep three storage and compute choices independent. Recording silence removal shortens saved audio. Opus DTX can save bytes during pauses. Transcription speech gating can reduce audio sent for recognition.
- Retain the native Windows interface, recording-only defaults, and scripted worker commands. These are proposed changes to v2.0.0.

## Capabilities

### New Capabilities

| Capability | Scope |
| --- | --- |
| [live-capture-session](specs/live-capture-session/spec.md) | Independent controls, shared capture, failure isolation, output intervals |
| [silence-and-time-mapping](specs/silence-and-time-mapping/spec.md) | Quiet speech, three clocks, gaps, compact recordings and playback |
| [remembered-programs](specs/remembered-programs/spec.md) | Recent programs, matching, restart and ambiguity handling |
| [live-recorder-interface](specs/live-recorder-interface/spec.md) | Native controls, output status, keyboard navigation and narrow windows |
| [live-transcription](specs/live-transcription/spec.md) | Chunked recognition, incremental text, recovery and finalization |
| [transcription-batch-queue](specs/transcription-batch-queue/spec.md) | Completed files, scheduling, cancellation and resumption |
| [live-speaker-attribution](specs/live-speaker-attribution/spec.md) | Optional local provisional labels and refinement |

### Modified Capabilities

- [manual-transcription](specs/manual-transcription/spec.md) makes the finished-file picker available during capture and preserves guided and scripted workflows.
- [recorder-resilience](specs/recorder-resilience/spec.md) defines which consumer stops on an audio writer failure.
- [speaker-attribution](specs/speaker-attribution/spec.md) defines the worker boundary during an explicitly requested live session.

## Impact

Implementation affects the Rust controller, mixer, worker, local files and operator guide. Optional models stay outside the portable ZIP. A compatible transcription endpoint can use ordinary multipart requests. No endpoint streaming extension is required.

Start with the [design](design.md), [acceptance plan](acceptance.md), and [implementation tasks](tasks.md).
