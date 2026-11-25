## Context

The released recorder handles source selection, WASAPI capture, encoding and native controls. The separate worker already handles custom endpoints, finished-file batches, cached responses, and optional offline speaker analysis. The proposed live mode reuses those boundaries. [Recording architecture](../../../docs/wiki/recording.md) · [Worker architecture](../../../docs/wiki/transcription.md)

## Goals / Non-Goals

A user can keep audio, text, or both. A slow recognizer cannot stop a healthy recording. A completed transcript must say where coverage is incomplete. Existing recording setups keep their settings. The first implementation targets near-live text using a compatible speech endpoint and optional local speaker analysis. Model installation, continuous background capture, and automatic memory ingestion are outside this change.

## Decisions

### Give each consumer its own controls

| User action | Audio file | Live text |
| --- | --- | --- |
| Start recording | Opens a new recording interval at the current frame | Keeps its current state |
| Start transcription | Keeps its current state | Shows scope and destination, then starts a new interval on confirmation |
| Stop recording | Closes the audio at the chosen frame | Continues if active |
| Stop transcription | Continues if active | Seals input at that frame, drains accepted work and finalizes its text |
| Stop both while both are active | Closes the audio | Uses the same cutoff and finishes pending text |

On first launch, leave live transcription, automatic program selection and speaker labels off. Keep the regular Opus 32 kbps mono recording default. Show the selected program and mic above the two start buttons. Place speaker labels and speech gating with transcription. Place codec and recording silence settings with recording. Keep finished-file batches accessible from either state. [UI requirements](specs/live-recorder-interface/spec.md)

Use one monotonic 48 kHz session clock and half-open frame ranges. Pin the selected sources for each active interval. Changing the destination, model or approved live sources requires a new consent scope. Label every recording and transcript interval separately. A shared identifier links overlapping outputs even when one file finishes first.

### Keep speech processing off the capture path

Reuse the timestamped blocks in `record.rs`. Tap per-source audio before recording pause, silence removal and encoding. Feed a bounded, nonblocking queue. A worker-side thread handles conversion, transient chunks and IPC. Run HTTP, inference, transcript writes and disk recovery outside the capture and UI threads. Limit source subscriptions and buffered bytes before admitting work.

Use a local, per-session IPC channel restricted to the current user. A versioned envelope carries the session, interval, source, first frame, frame count and sequence. Include cutoff, discontinuity, gap and completion events. Batch adjacent capture blocks for IPC. Consumers reject stale session IDs and duplicate sequences. The capture path reports a gap if the worker cannot keep up. Reuse the existing capture path for recording alone, without a worker or speech buffer.

For compatible endpoints, issue normal bounded multipart requests through the worker. Reuse the existing base URL, authentication, size and redirect rules. Publish committed segments in source-time order. Optional context overlap can help recognition at boundaries. Deduplicate only when time and provenance support it. Preserve real repeated words. Show uncertain timing explicitly.

### Treat text as the primary output of transcription

Start a private, user-local cache on affirmative live start. Use a small append-only journal and a human-readable UTF-8 partial text file. Store sequence, source, frame bounds, recognition state and speaker-label revision separately from spoken text. Keep the preview window bounded. The full transcript remains readable on disk.

Stopping transcription closes its own interval. Flush accepted work, then publish a collision-safe final TXT in the chosen folder. Default to the recording output folder, including for a text-only session. Stage the final file in the destination directory before an atomic rename. Keep the recoverable cache on a destination failure. If the deadline expires, mark missing or unresolved coverage in the result. Later recovery produces a distinct revision. [Durability and recovery](specs/live-transcription/spec.md)

For example, start Discord transcription at 0 seconds, start recording at 5, and stop recording at 12. Stop transcription at 20. The audio covers [5, 12) and the text covers [0, 20). Text outside the recording has a session time and no playable file offset. With concurrent starts and **Stop both**, the recording and transcript share their logical bounds. A worker can finish its last request after the recording closes.

### Make silence and speaker processing optional

Recording silence removal and Opus DTX serve different needs. Apply the existing shared recording gate to all saved tracks together. Preserve its time map when frames are removed. Apply an optional speech gate separately to each recognition source. Retain speech margins and uncertain quiet input. Text-only mode keeps no full-call audio by default. TXT size depends primarily on the recognized words. [Clock and silence examples](specs/silence-and-time-mapping/spec.md)

The v2.0.0 local adapter accepts finished audio. Prototype a warm, bounded-window adapter in the worker before enabling live labels. Measure label delay, speaker continuity, overlap error, memory and inference speed on the same pinned fixtures. Unknown or provisional speakers keep their transcript text. An optional final pass can refine only intervals with retained authorized audio. Suggested identities still require the existing naming and export checks. [Speaker requirements](specs/live-speaker-attribution/spec.md)

### Reuse the finished-file worker for batches

Queue only selected, finalized inputs. Snapshot options and consent for that batch. Give the UI's live and batch requests one on-demand worker owner. A guided console submits approved work over local IPC. It must not start a second independent network queue. Exit the owner when live work, runnable batches and pending finalizations have ended.

Start with one request slot per destination. If a batch owns it, show a cancellable waiting state before admitting live audio. Reserve one slot throughout an active live interval. At the default limit, queued batches resume after live requests finish. A manually launched standalone CLI keeps its existing behavior and job locks. Its traffic is outside the UI scheduler's concurrency guarantee. Reuse v2.0.0 cache keys and explicit retries after an unknown remote outcome. [Queue requirements](specs/transcription-batch-queue/spec.md)

## Risks and tradeoffs

| Risk | Required behavior |
| --- | --- |
| Quiet words disappear or timestamps jump after compaction | Default recognition gating off. Test quiet words and map source frames to retained file frames. |
| A remote endpoint or local model runs slower than the call | Bound queued audio, expose lag, protect recording and identify missing ranges. |
| Overlapping Discord voices exchange labels | Keep separate source evidence, abstain when uncertain and evaluate mixed calls separately. |
| Crash, low disk space or a full output folder | Preserve committed text, release capture promptly and offer local recovery without silent resubmission. |
| A saved program name matches several processes | Require a unique verified identity, or present the candidates for explicit selection. |

## Migration and delivery

Keep current preference files, named setups, finished-file CLI arguments, offline sessions and memory exports readable. Introduce versioned live artifacts alongside them. Existing presets cannot authorize upload, start live capture or load a model. Ship the independent controls and cache before enabling optional speech gating or live speaker labels. Apply the [release gates](acceptance.md) to each capability.

The released baseline is represented by change directories. Materialize `add-transcription-workflow` before `integrate-recorder-and-transcription`. Also materialize `improve-recorder-reliability-and-efficiency` and `add-speaker-attribution-and-memory-export`. Give the predecessor canonical specs meaningful Purpose sections if the archive tool generates placeholders. Then apply the modified requirements here. This proposal leaves the released change records intact.
