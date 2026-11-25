## Why

A user selected an MP4, then entered a service URL, model, and output settings in a second window. A stopped local Whisper service caused the approved batch to fail before any audio reached recognition. The existing file picker and Rust decoder already extracted the right track. Setup and control broke the workflow.

## What Changes

- Complete every supported operation in the Windows application. Use native forms for settings, files, queue, transcript review, speakers, recovery, and memory.
- Offer on-demand local speech recognition and speaker processing implemented in Rust. Keep model weights separately installable. Keep explicit custom OpenAI-compatible endpoints available.
- Give agents a noninteractive JSON command interface and a local MCP server. Both control the same application state as the UI, including recording.
- Preserve independent recording and transcription, MP4 extraction, source timelines, batch recovery, configuration, and existing script compatibility.
- Keep setup status distinct from model readiness and endpoint reachability. Show actionable failures and the scope of a retry.

## Capabilities

### New Capabilities

| Capability | User outcome |
| --- | --- |
| [Native workflow UI](specs/native-workspace/spec.md) | A user completes each workflow without a terminal, shell script, or browser. |
| [Native speech runtime](specs/native-speech-runtime/spec.md) | Local recognition and qualified speaker processing run offline after model installation. |
| [Local agent control](specs/local-agent-control/spec.md) | An authorized agent uses the CLI or MCP to operate the same recorder and jobs. |

### Modified Capabilities

- [Manual transcription](specs/manual-transcription/spec.md) replaces GUI console prompts with native review and submission. Explicitly invoked legacy scripts retain their interfaces.

## Impact

This proposal affects the two Rust executables, their shared application logic, profiles, model distribution, and Windows tests. It introduces a local control protocol. Model selection and performance remain subject to qualification.

Read the [design](design.md), [control contract](control.md), [acceptance tests](acceptance.md), and [implementation checklist](tasks.md).
