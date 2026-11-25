# Transcription

[Wiki home](README.md) · [Operator guide](../../rust/README.md) · [Focused checks](verification.md)

## Offline MP4

Open **Transcribe files…** to import the English model pack, preview one completed MP4, approve the job, and review timed text. The GUI, CLI, and MCP server share the local job service. [Operator steps](../../rust/README.md#transcribe-an-mp4-offline) · [Service](../../rust/src/bin/transcribe/local_batch.rs) · [Win32 panel](../../rust/src/native_controller/local_batch.rs)

The local worker opens the first audio track through Windows Media Foundation. It passes bounded 16 kHz mono PCM windows to the CPU recognizer. Selection, model discovery, and preview leave inference idle. The operator can import the pinned English pack from disk. Pack verification checks the configuration, tokenizer, and weights. [Decoder](../../rust/src/bin/transcribe/video.rs) · [Recognizer](../../rust/src/bin/transcribe/local_asr.rs) · [Pack verification](../../rust/src/bin/transcribe/model_pack.rs)

Each approved job writes `<output>/<batch_id>/transcript.txt` and `transcript.json` together. The JSON records the input hash, model revision, decoded duration, and segment boundaries in milliseconds. Poll job status for counts. Request transcript pages separately to read text. One job per state directory can load the model at a time. [Runner and publisher](../../rust/src/bin/transcribe/local_batch/runner.rs)

Use `audio-capture.exe control` for a headless launcher that needs a windowless JSON CLI. Use `audio-capture.exe mcp` for an MCP host. Both accept redirected pipes. Existing terminal scripts can use `transcribe.exe control` with the same arguments. [Windows entry points](../../rust/src/main.rs)

The operator grants access to selected local MP4 paths and output folders. Preview binds the job to its inputs and settings. Commit requires approval and an idempotency key. Approved work continues after the client disconnects. Job state defaults to `%LOCALAPPDATA%\AudioCaptureRust\native-control-v1`. [Agent setup](../../rust/README.md#control-a-local-mp4-job) · [Grant policy and storage](../../rust/src/bin/transcribe/local_batch/) · [MCP handlers](../../rust/src/bin/transcribe/control/mcp.rs)

## Endpoint consent

Endpoint batches use the explicitly selected compatible provider. Guided mode shows the selected paths, effective settings, and upload destination before submission. [Worker launcher](../../rust/src/worker_launch.rs) · [Guided prompts](../../rust/src/bin/transcribe/guided.rs)

Scripted mode starts eligible work on invocation. It takes options from arguments and reads any bearer token from the named environment variable. Recording alone starts no speech worker or recognition request. [Worker entry point](../../rust/src/bin/transcribe/main.rs) · [Integration specification](../../openspec/changes/add-independent-live-transcription/specs/manual-transcription/spec.md)

## Live text and batch scheduling

The recorder opens a worker after the user approves the scope. The worker admits one live interval and then reads versioned frames over its inherited stdin pipe. The wire uses stable session and interval IDs, numbered sources and 48 kHz source offsets. The recorder's relay converts per-source stereo audio to mono PCM16 outside the capture thread. The worker reports status through stdout. [Wire and bounded relay](../../rust/src/live_wire.rs) · [Live worker](../../rust/src/bin/transcribe/live.rs)

Each frame starts with `ACL1`, a one-byte kind, and a little-endian `u32` payload length. Kind 0 contains a UTF-8 JSON control event of up to 64 KiB. Kind 1 contains a little-endian source `u16`, first frame `u64`, and up to 4,800 signed PCM16 samples. The Start event binds ordered sources to the approved session ID, interval ID, and initial frame. Gap, timing, speaker, overlap, and end events carry positions on the same clock.

The worker groups samples into bounded WAV requests. It stages the WAV in memory. During capture it records requests, completed text, and gaps under `%LOCALAPPDATA%\AudioCaptureRust\live-v1`. The readable `transcript.partial.txt` grows as results commit. Stopping transcription seals the interval and starts finalization. [Live cache](../../rust/src/bin/transcribe/live/cache.rs) · [Live transcript and recovery guide](../live-transcription.md)

The version 1 JSONL index starts with a `begin` event that binds the session, interval, and initial source frame. Sequence numbers order events. Request, speech, gap, and skip events identify ranges by source, start frame, and exclusive end frame. Recording events identify saved audio intervals. A checkpoint tracks the durable text and journal offsets. An end event stores the cutoff and incomplete status. [Journal](../../rust/src/bin/transcribe/live/cache/journal.rs)

Endpoint batches persist in `%LOCALAPPDATA%\AudioCaptureRust\batch-queue`. Admission leases limit live and batch requests. Live intervals reserve capacity. Batch state holds the credential variable name. The active process reads its value from the environment. [Queue implementation](../../rust/src/bin/transcribe/batch_queue.rs) · [Queue rules](../../openspec/changes/add-independent-live-transcription/specs/transcription-batch-queue/spec.md)

The default admission limits allow one request overall and one per destination. For controlled deployments, set `total` and `per_destination` in `batch-queue/admission/limits.json`. Check the configured endpoint's capacity before raising limits. Scripted recognition that starts outside a guided batch retains its standalone request behavior.

## Transfer and local state

| Concern | Code and behavior |
| --- | --- |
| Endpoint | [`transport::endpoint`](../../rust/src/bin/transcribe/transport.rs) keeps the base URL's path prefix and appends `/audio/transcriptions`. HTTPS and loopback HTTP work by default. Only scripted mode can enable remote HTTP with `--allow-http`. |
| Request and response | [`transport::transcribe`](../../rust/src/bin/transcribe/transport.rs) uses WinHTTP for streamed multipart requests. It rejects redirects and caps response bodies at 16 MiB. [`parse_response`](../../rust/src/bin/transcribe/jobs.rs) accepts UTF-8 text or JSON with a string `text` field. Use `--timestamps` for `verbose_json` or select the known diarized provider profile. |
| Batch and size | [`jobs::run` and `process_file`](../../rust/src/bin/transcribe/jobs.rs) process files in order. The default limit is 25 MiB per upload. Oversized audio files require the split option. Video imports always apply the limit to decoded audio. |
| Video import | The Windows source reader selects the first embedded audio track and streams mono PCM16 at 16 kHz. Timing starts at its first decoded sample. The conversion method participates in cache identity. [Rust decoder](../../rust/src/bin/transcribe/video.rs) · [OpenSpec](../../openspec/changes/add-native-video-audio-ingest/specs/manual-transcription/spec.md) |
| Chunking | The worker stages one bounded WAV part at a time. FFmpeg handles optional conversion of oversized audio files. Native video import uses the same WAV writer. [Chunk writer](../../rust/src/bin/transcribe/chunks.rs) |
| Cache | [`process_part`](../../rust/src/bin/transcribe/jobs.rs) keeps locked job state and buffered response reads under `.audio-capture-jobs` in the output folder. Matching completed work reuses local output. An interrupted local write can reuse a saved response. |
| Retry | Before an upload, the worker saves the `unknown` state. If the endpoint may have processed a request and no response is available locally, the worker requires `--retry-unknown` to send it again. [`process_part`](../../rust/src/bin/transcribe/jobs.rs) |

Resume with the same output folder, source paths and bytes, destination, model, and request options. Chunked jobs also depend on chunk identity and the configured file limit. Check the endpoint before you retry an unknown result. [Job identity and state](../../rust/src/bin/transcribe/jobs.rs)

## Review and resource boundaries

- Use `--same-session` only for tracks on one recording timeline. The worker rejects a recognized mixed file with its separate tracks unless scripted mode sets `--allow-duplicate-sources`. [Batch validation](../../rust/src/bin/transcribe/jobs.rs)
- The worker keeps the transcript text if it cannot validate timing. It checks whole-file duration with ffprobe and uses the decoded frame count for chunks. [Timing validation](../../rust/src/bin/transcribe/jobs.rs)
- The local HTML uses the original media paths for playback. It supports search, timed replay, and export of corrections to separate files. Use a new output path for each review. [Review generator](../../rust/src/bin/transcribe/review.rs) · [Review page](../../rust/src/bin/transcribe/review.html)
- Endpoint jobs use Windows codecs for video import. Optional external tools handle oversized audio files and whole-file duration checks on that path. [Build guide](../../BUILD.md) · [Video wire test](../../rust/tools/test_video_transcription.py)
- The local MP4 acceptance test exercises the packaged UI, JSON CLI, and stdio MCP. It uses a private Windows desktop for native controls and compares timed output across all three runs. [Acceptance harness](../../rust/tools/test_local_mp4.py)

## Speaker attribution and memory

The operator can prepare, attribute, review, and export a session using completed audio and recognition artifacts. These commands run in the companion worker on request. [Speaker workflow](../speaker-workflow.md)

| Concern | Implementation |
| --- | --- |
| Command routing | The local subcommands have separate arguments from the existing recognition commands. Use `prepare-session` to create a manifest and `attribute` to process one or more manifests. [Command parser](../../rust/src/bin/transcribe/attribution/cli.rs) |
| Timeline and text | The worker aligns recognized text and speaker activity on the recorded timeline. It preserves untimed or unresolved text and retains activity at overlapping times. [Inputs](../../rust/src/bin/transcribe/attribution/inputs.rs) · [Session model](../../rust/src/bin/transcribe/attribution/model.rs) |
| Local backend | An explicit adapter reads selected audio through a bounded subprocess protocol. The provided Python bridge runs sherpa-onnx models installed by the operator. [Rust boundary](../../rust/src/bin/transcribe/attribution/adapter.rs) · [Python bridge](../../rust/tools/diarize_local.py) |
| Context and changes | The naming rules use local evidence. Review patches target a revision and preserve earlier decisions. Speaker IDs and reviewed person IDs serve different purposes. [Naming](../../rust/src/bin/transcribe/attribution/naming.rs) · [Review](../../rust/src/bin/transcribe/attribution/decisions.rs) · [Revision store](../../rust/src/bin/transcribe/attribution/store.rs) |
| Memory | Preview an explicit scope. Export approved data as versioned JSONL batches. The reference importer checks the manifest and replays corrections or withdrawals. [Export](../../rust/src/bin/transcribe/attribution/export.rs) · [Import](../../rust/src/bin/transcribe/attribution/importer.rs) |

The [OpenSpec design](../../openspec/changes/add-speaker-attribution-and-memory-export/design.md) explains the protocol, evidence rules, revisions, and acceptance criteria. See the [verification notes](verification.md) for runnable checks and evidence limits.
