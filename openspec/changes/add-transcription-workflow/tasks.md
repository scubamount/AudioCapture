# Implementation tasks

## 1. Protocol and local batch

- [x] 1.1 Implement an explicit standalone command with a base URL, model, file list, output location, and optional authentication.
- [ ] 1.2 Stream bounded multipart audio. Test reverse-proxy prefixes, MIME types, redirects, timeouts, and local HTTP behavior.
- [ ] 1.3 Preserve per-input source identity and raw responses. Test duplicate names, existing outputs, and malformed responses.
- [x] 1.4 Persist job outcomes and distinguish unknown remote completion from known local failure. Test explicit resume and retry.
- [x] 1.5 Report oversized files unless independently decodable chunking and source-relative offsets are validated.
- [x] 1.6 Add explicit long-file chunking with installed FFmpeg and a bounded 16 kHz mono PCM temporary chunk. Preserve originals, offsets, raw outputs, and durable per-chunk status.
- [x] 1.7 Verify a synthetic two-hour source with measured peak memory and temporary disk use. Test boundary offsets, partial failure, restart, and an unknown upload outcome.

## 2. Review and source timing

- [x] 2.1 Validate returned times against the decoded source duration and configured units.
- [x] 2.2 Generate local source-labeled search and replay with simultaneous segments retained.
- [x] 2.3 Export corrections separately and test escaping, untimed text, local media access, and output collisions.
- [x] 2.4 Merge source times only for a confirmed shared session. Test unrelated files in one batch.

## 3. Integration and acceptance

- [x] 3.1 Include the companion command in the verified portable Windows package and document its exact invocation.
- [x] 3.2 Exercise a local synthetic endpoint end to end with two independent sources and assert both text results.
- [ ] 3.3 Record a pinned compatibility matrix for Speaches and LocalAI when each service is available. Record whisper.cpp compatibility through an adapter only when tested.
- [ ] 3.4 Measure batch memory and throughput at increasing input sizes. Report provider limits and untested quality cases.
- [ ] 3.5 Run the controlled quality spike before selecting a model, speech detection profile, or automatic correction.

## Verification so far

A native local request sent two separate synthetic 32 kbps Opus sources through a prefixed faster-whisper endpoint. Both resulting transcripts matched their reference phrases. The local review smoke test exercised source timing and text escaping. Audible playback remains unverified.

A two-hour synthetic source produced nine requests with a 25 MiB limit and one temporary part at a time. The packaged worker used 9.86 MiB peak working memory. FFmpeg used 25.02 MiB. The mock endpoint acknowledged all 115,200,000 frames in 2.658 seconds. A completed repeat took 0.169 seconds with zero uploads and FFmpeg hidden. A disconnected request required explicit retry. The worker reused the completed first chunk. These figures measure the batch pipeline and exclude speech inference time.

The portable package contains both release executables and the locked dependency inventory. Recovery tests cover interrupted aggregate writes, existing output collisions, and reuse of saved responses. The review keeps unrelated recordings on separate timelines and can align an explicitly selected shared session.

A local faster-whisper endpoint also transcribed a previously recorded 29.088-second Discord and microphone conversation. Both sources produced labeled speech and in-range timestamps. A second local decode differed on the Discord wording. Hosted service compatibility and representative conversational accuracy remain open.
