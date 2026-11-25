## User workflows

**Transcribe a video.** Select **Transcribe files…**, add an MP4, and choose the local model or a configured endpoint. The window shows the first audio track, model, output folder, timestamps and review choices. Select **Transcribe** once. Follow progress in **Queue**, then open the text or timed review. An installed local model needs no URL, key, port, or separate server.

**Record a call.** Select the program and microphone. Start recording, live transcription, or both. Stopping either activity leaves the other running. The window shows which sources are active, the text committed so far, gaps, and the final output paths.

**Run unattended.** An authorized agent lists sources, previews the operation, and submits it using its plan ID. It receives a durable handle before long work finishes. The GUI can show the same operation. The agent reads the results through the documented control interface, even if no window exists. [Control contract](control.md)

### Coverage by task

| Task | Native interaction | Agent equivalent |
| --- | --- | --- |
| Setup | Choose local or compatible endpoint, install or import models, set language and output, test service, load or save a portable profile | Manage profile and model settings, inspect capabilities and run an explicit readiness check |
| Source selection | List programs and devices, select recent programs, manage matching, choose a mic or system mix, routes, gains and monitoring | Enumerate and resolve exact sources, edit routing, recents and preferences |
| Record and live text | Set presets, codec, bitrate, WAV rate, channels, DTX, silence removal, file grouping and folder. Start, pause, resume, mark or stop where supported | Use the same settings and session transitions, retrieve levels, gaps, live text and final output references |
| Completed files | Select audio, video or recording timelines. Choose stems or mix, alignment, timestamps, split policy, language, optional speaker settings and HTML export | Preview identical selection and effective settings, submit once and track job and file results |
| Queue and recovery | See progress and failure details, pause, resume, cancel, resolve unknown outcomes, recover or discard an interrupted transcript | Use job and recovery identifiers with the same confirmation and retry rules |
| Review | Read, search, seek or replay on request, correct segments, export corrections, open or reveal outputs | Read paged segments, resolve time and media spans, manage reviewed edits and exports. Playback is a separately authorized action |
| Speakers | Prepare or refine sessions, select sources and models, inspect overlaps, review names and evidence, merge or correct assignments | Supply structured source mappings and session IDs. Propose or apply a revision with provenance |
| Memory | Select approved sessions and fields, preview counts and exclusions, export or import a revisioned batch | Preview or commit the same scope. Replay corrections or withdrawals using the existing memory format |

The UI can use a main recording view, a transcription workspace with Queue and Review, and a settings dialog. Keep recovery and memory operations accessible from their subject. File pickers, drag selection and keyboard actions must have structured equivalents. Avoid a generic command-entry field as a substitute for native forms.

## Architecture

Keep the existing two public executables. Move reusable operations out of window callbacks and terminal input loops into typed Rust application services. Link the recorder to capture and presentation code. Keep inference dependencies in the on-demand worker or a verified native component it alone loads.

```text
Windows UI       transcribe.exe control ... --json       transcribe.exe mcp
     \                         |                              /
                  Typed Rust commands and results
                              |
             One session owner per logon and state directory
                 |                            |
         WASAPI capture and writers       Job scheduler + journal
                                              |
                                 Local Rust models or configured endpoint
```

Use one session owner for device and output lifetimes. A hidden process can own the session when no GUI exists. Attach the GUI and agent to the same owner. Serialize mutations per resource. Give the capture thread bounded messages and buffers. File hashing, model loading, decoding, recognition, and durable writes run off that thread.

The owner key includes the Windows logon and resolved application state directory. Two portable clients that share the default directory must negotiate a compatible protocol with the same owner. An incompatible client reports the version mismatch before opening devices or writing queue state.

The worker currently holds the queue, cache, speaker and recovery domain code. Replace interactive I/O at its edges. Submit durable jobs separately from driving the queue. Detect child failure and finalize remaining consumers independently. Use inherited pipes or authenticated local IPC for worker traffic. Suppress console windows throughout the process tree. [Current recorder launch](../../../rust/src/worker_launch.rs) · [Queue implementation](../../../rust/src/bin/transcribe/batch_queue.rs)

Approved batches survive a UI or MCP client disconnect. Interrupted work resumes from durable state under existing retry rules. A headless live capture uses an explicit lease. Loss of its last owner stops and finalizes that interval after expiry. A client disconnect leaves independently owned capture intact. Closing the final visible window offers an explicit choice for ongoing work.

### Local and endpoint processing

Reuse native audio extraction, frame clocks, source labels, and bounded chunking across providers. Send local inference decoded frames directly. Encode a bounded request for a selected remote endpoint. A remote profile may pass supported audio through directly. A video always contributes audio only. Implement built-in audio decoding, duration checks, VAD and diarization through Rust code or the documented Windows media APIs. The standard workflows must work on a clean installation without Python, FFmpeg, ffprobe, a model server, or a command interpreter.

Qualify an English CPU model comparable to Whisper `base.en` first. Evaluate a multilingual profile separately. Candle Whisper is a candidate for Rust inference. Validate model format, supported operators, timestamp decoding, quantization, and Windows performance using the exact weights and build. Evaluate a Rust ONNX executor such as tract for segmentation and embeddings. A C/C++ inference engine accessed through Rust bindings is a comparison baseline. Adopting it for a built-in backend requires an explicit spec decision. Existing Windows APIs and already-shipped recording codecs remain supported. [Candle Whisper example](https://github.com/huggingface/candle/tree/main/candle-examples/examples/whisper) · [tract](https://github.com/sonos/tract)

Model packs declare task, architecture, format, languages, revision, hash, download size, installed size, license, and required runtime version. Installing requires a user action or scoped agent grant. Support offline import. Verify all assets before atomic activation. A failed install preserves the working version. An active job pins its model revision. Measure memory through release after idle and after a backend change.

Separate program and mic tracks can supply immediate source labels. Multiple people on one track require qualified diarization. Keep overlapping turns, unknown speakers and provisional labels. Apply existing evidence and review rules to names. Treat export to agent memory as its own approved operation. [Speaker workflow](../../../docs/speaker-workflow.md)

### Configuration and data compatibility

Use a versioned profile with a discriminated local or endpoint provider. A local profile references an installed model pack. An endpoint profile keeps the complete base URL, model ID, credential reference and request capabilities. Display the resolved route for endpoint work. A configured endpoint can be offline.

On a clean installation, preselect **Local**. If no compatible pack is installed, show **Install model** and **Import model**. Automatic setup must never choose a remote destination. Record which configuration and model revision each approved job used.

Read existing version 1 profiles as endpoint profiles, including custom URL prefixes and the private loopback Whisper profile. Keep the API-key variable name and config-relative paths. Offer **Switch to native local** with a preview. Validate compatible weights and write a new profile only on save. The existing faster-whisper `base.en` package uses CTranslate2 weights. A matching model name alone cannot establish format compatibility. [Model card](https://huggingface.co/Systran/faster-whisper-base.en)

Keep text, segment, timeline, speaker-revision and memory formats readable. Preserve completed caches for equivalent endpoint jobs. Include provider, model revision, extraction policy and recognition settings in new local job identities. Display older results with their recorded provenance. Prompt before repeating any request with an unknown remote outcome.

### Rollout and spec precedence

The canonical manual spec and earlier open changes describe a guided console. This change replaces that GUI entry point. Explicit terminal scripts keep their existing CLI semantics. Apply the independent-live and native-video manual deltas first, then apply this change. Rehearse this order in a disposable spec tree. Existing privacy, source-time, recovery and naming guarantees continue to apply. [Canonical baseline](../../specs/manual-transcription/spec.md) · [Live predecessor](../add-independent-live-transcription/specs/manual-transcription/spec.md) · [Video predecessor](../add-native-video-audio-ingest/specs/manual-transcription/spec.md)

Deliver the first vertical slice with one MP4, an installed local model, one native Submit action, and corresponding CLI and MCP actions. Expand from this slice using the parity table. Mark unavailable features consistently through capabilities. Declare full parity only after every row passes through all applicable clients.
