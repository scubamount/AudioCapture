# Design

## Entry point

The user invokes a separate Rust executable with completed input files, an output directory, a model identifier, and an endpoint base URL. A command may specify one file or a batch. Listing, recording, stopping, and opening audio do not initiate transcription.

```powershell
.\transcribe.exe --base-url http://127.0.0.1:8000/v1 --model local-asr --output-dir .\transcripts .\Discord.opus .\Microphone.opus
```

The command is an implementation target. The exact packaged binary and switches must be verified against `--help`. An endpoint must be running before a request. A local endpoint needs no key. An optional key is read from an explicitly named environment variable. A future GUI entry may invoke the same contract after an explicit user action.

## Endpoint contract

Append `/audio/transcriptions` once to the configured base path. Preserve a reverse-proxy prefix. Reject embedded credentials, queries, and fragments. Use a streamed multipart request with one file part and a model field. Language and timestamp formats are explicitly selected optional fields. A provider's rejection of a selected option is visible to the user.

Use verified HTTPS for remote services. Permit loopback HTTP and an explicit user-selected HTTP endpoint for a trusted network. Do not follow redirects with the media or authorization. Keep keys out of command examples, result metadata, and error logs. Enforce bounded input and response sizes, timeouts, and per-file errors.

Accept compatible plain-text responses and JSON containing a text field. Preserve the JSON response. Track segments only when the response identifies valid timestamps and their units. Compare positions with decoded source duration where available. Unknown, inconsistent, or out-of-range times remain untimed. Do not reinterpret an unexplained scale. A source label identifies the recording track, not necessarily an individual person.

Speaches and LocalAI document compatible audio transcription routes. The whisper.cpp example server uses `/inference` and needs an adapter or compatible wrapper. A deployed provider, version, model, media type, optional parameter, and timestamp unit require their own verification. Record a compatibility matrix for JSON, plain text, Ogg Opus, WAV, language, timing, malformed responses, and oversized files. No hosted service was tested in the prior local mock.

## Batch identity and results

Process files with bounded memory and bounded concurrency. Each result retains an unambiguous source identity, model and endpoint identity, effective options, input fingerprint, and job status. Exclude credentials. Use exclusive or atomic output writes and do not replace an existing transcript or source recording. A second file with the same basename must remain distinct.

Persist completed work so a resumed batch can keep its successful results. An interruption or timeout after upload has an unknown remote outcome. Record that state and require an explicit retry before resending it. A known local validation failure can be corrected and retried. Report each result independently, including partial failures. A batch is a client queue of ordinary transcription requests and does not require a provider batch API.

For input too large for the configured endpoint, report the item as too large unless the user explicitly enables long-file chunking. The opt-in chunker may use an installed FFmpeg to produce bounded 16 kHz mono PCM chunks. Process one temporary chunk at a time, and retain its source offset, raw result, and durable state. Convert valid segment times back to the source timeline. A timeout after upload remains unknown and requires an explicit retry. On partial failure, report the affected chunk and retain completed chunks and other items. Clean temporary media when it is no longer needed. Do not send a truncated file or change original audio.

Test a synthetic two-hour source with bounded memory and disk usage. Verify chunk boundaries, source offsets, restart, unknown remote state, and partial-failure reporting. Measure actual peak memory, temporary disk use, and processing time. An unavailable FFmpeg must produce a clear error before any chunk is uploaded.

## Review

Generate a local review using only explicit source files and response data. When sources have a known shared session and valid source-relative segment times, list them in time order while retaining overlaps. Keep unrelated sessions in separate timelines. A user can search, select a timed segment, and replay the corresponding source interval. Untimed responses remain searchable and identify their source without a fabricated sequence.

Display provider text as text, not executable markup. Keep the provider's raw result unchanged. Save or export the user's corrections separately. When files cannot be opened from a browser's local-file policy, report the limitation and preserve the text results. A replay must not make a transcription request.

## Quality and future decisions

Use the existing spike for a held-out comparison of faster-whisper VAD padding, short-turn recall, quiet speech, silence, model versions, word error rate, and timing. Compare stronger models only when their resources and identities are known. Keep WhisperX forced alignment optional and preserve untimed words. Evaluate any text correction separately against the raw transcript and reference.

The recorder may offer a user-selected mono Opus speech profile with VOIP and DTX. Its quality on representative calls remains a separate evaluation. Matroska session packaging, automatic recognition, automatic model downloads, and automatic quality correction remain evaluation topics. They are not required by this change. The separately supplied prototype establishes a loopback multipart example only. It does not establish real provider compatibility.

## Verification

Use a local mock with generated audio and explicit commands. Test custom prefixes, authentication, multipart fields, plain text and JSON, malformed responses, unsupported formats, redirect refusal, duplicate basenames, output collisions, timeout ambiguity, restart, and per-item continuation. Verify timing with known source durations and deliberately invalid units. Exercise review search, overlap, audio selection, text escaping, editing, and export.

Run real endpoint checks only with a configured service and authorized audio. Record the exact version, model, accepted settings, and results. A missing provider or held-out reference remains unverified. Measure peak memory and elapsed time across increasing file and batch sizes before making scalability claims.
