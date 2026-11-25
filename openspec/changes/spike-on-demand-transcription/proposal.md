# On-demand transcription quality spike

## Why

The recorder produces separate application and microphone tracks. Users need a deliberate way to transcribe one recording or a batch. The first Discord test also showed that signal quality and model choice affect the result.

## What Changes

- Evaluate a local baseline, stronger speech models, and optional text correction against the same labeled audio.
- Specify an on-command workflow for files and recording sessions. A batch contains explicitly selected items.
- Evaluate custom OpenAI-compatible audio transcription endpoints, model IDs, and optional credentials.
- Preserve source labels and recording-time offsets when merging timed results. Mark unsupported capabilities and uncertain grouping.
- Produce a recommendation, measurements, and an implementation proposal after the spike.

## Capabilities

### New Capabilities

- `transcription-evaluation` covers the spike, endpoint contract, queue behavior, output, and quality decision.

### Modified Capabilities

None. This spike consumes completed recorder outputs.

## Impact

The spike uses synthetic or explicitly selected evaluation audio. It may use a local test service and a local model. Any external endpoint run requires an explicit user command. The spike does not add recorder dependencies or Rig. Product implementation follows a successful evaluation.
