# Transcription

[Wiki home](README.md) · [Operator guide](../../rust/README.md) · [Focused checks](verification.md)

## Entry point and consent

1. While idle, the recorder enables **Transcribe files…**. The native picker accepts multiple files and uses the worker's shared extension list. Canceling the picker ends the action. [Controller](../../rust/src/native_controller.rs) · [Extension list](../../rust/src/lib.rs)
2. The recorder starts its sibling `transcribe.exe` in a separate interactive console. It passes `--guided --` followed by one argument per selected path. [Worker launcher](../../rust/src/worker_launch.rs)
3. Guided mode asks for the destination, model, output folder, and optional API key environment variable name. It shows the selected paths and effective settings. Only a complete affirmative answer at the final prompt starts the batch. [Guided prompts](../../rust/src/bin/transcribe/guided.rs)

Scripted mode starts eligible work on invocation. It takes options from arguments and reads any bearer token from the named environment variable. The recorder itself makes zero transcription requests during recording. [Worker entry point](../../rust/src/bin/transcribe/main.rs) · [Integration specification](../../openspec/changes/integrate-recorder-and-transcription/specs/manual-transcription/spec.md)

## Transfer and local state

| Concern | Code and behavior |
| --- | --- |
| Endpoint | [`transport::endpoint`](../../rust/src/bin/transcribe/transport.rs) keeps the base URL's path prefix and appends `/audio/transcriptions`. HTTPS and loopback HTTP work by default. Only scripted mode can enable remote HTTP with `--allow-http`. |
| Request and response | [`transport::transcribe`](../../rust/src/bin/transcribe/transport.rs) uses WinHTTP for streamed multipart requests. It rejects redirects and caps response bodies at 16 MiB. [`parse_response`](../../rust/src/bin/transcribe/jobs.rs) accepts UTF-8 text or JSON with a string `text` field. The `--timestamps` option requests `verbose_json`. |
| Batch and size | [`jobs::run` and `process_file`](../../rust/src/bin/transcribe/jobs.rs) process files in order. The default file limit is 25 MiB. With chunking off, metadata rejects oversized files before hashing or probing. Other files in the batch can continue. |
| Chunking | [`chunks::for_each_wav`](../../rust/src/bin/transcribe/chunks.rs) uses FFmpeg only for oversized inputs when the user enables chunking. It writes one bounded 16 kHz mono PCM WAV at a time. Chunk offsets count frames on the source timeline. |
| Cache | [`process_part`](../../rust/src/bin/transcribe/jobs.rs) keeps locked job state and buffered response reads under `.audio-capture-jobs` in the output folder. Matching completed work reuses local output. An interrupted local write can reuse a saved response. |
| Retry | Before an upload, the worker saves the `unknown` state. If the endpoint may have processed a request and no response is available locally, the worker requires `--retry-unknown` to send it again. [`process_part`](../../rust/src/bin/transcribe/jobs.rs) |

Resume with the same output folder, source paths and bytes, destination, model, and request options. Chunked jobs also depend on chunk identity and the configured file limit. Check the endpoint before you retry an unknown result. [Job identity and state](../../rust/src/bin/transcribe/jobs.rs)

## Review and resource boundaries

- Use `--same-session` only for tracks on one recording timeline. The worker rejects a recognized mixed file with its separate tracks unless scripted mode sets `--allow-duplicate-sources`. [Batch validation](../../rust/src/bin/transcribe/jobs.rs)
- The worker keeps the transcript text if it cannot validate timing. It checks whole-file duration with ffprobe and uses the decoded frame count for chunks. [Timing validation](../../rust/src/bin/transcribe/jobs.rs)
- The local HTML uses the original media paths for playback. It supports search, timed replay, and export of corrections to separate files. Use a new output path for each review. [Review generator](../../rust/src/bin/transcribe/review.rs) · [Review page](../../rust/src/bin/transcribe/review.html)
- The portable ZIP includes the recorder and worker. The endpoint supplies recognition. Install FFmpeg for oversized-file conversion and ffprobe for whole-file timing validation. [Build guide](../../BUILD.md) · [Portable delivery specification](../../openspec/changes/integrate-recorder-and-transcription/specs/manual-transcription/spec.md)

## Planned speaker attribution and export

Current review labels identify source tracks. A proposed change covers offline speaker diarization, reversible naming with evidence, and versioned local JSONL export for agent memory. [Proposal](../../openspec/changes/add-speaker-attribution-and-memory-export/proposal.md) · [Design](../../openspec/changes/add-speaker-attribution-and-memory-export/design.md)
