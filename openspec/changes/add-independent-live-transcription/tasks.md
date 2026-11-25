## 1. Establish the contracts and baseline

- [x] 1.1 Record the v2.0.0 reference build, hardware, corpus hashes and existing recording-only measurements described in [acceptance](acceptance.md).
- [x] 1.2 Add failing state-table, time-map and independent-stop tests. Cover rapid restart, both stop orders, pause and source loss.
- [x] 1.3 Define versioned IDs for sessions, recording intervals, transcript intervals, chunks, revisions and recovery checkpoints. Document the IPC and artifact formats.

## 2. Separate capture consumers

- [x] 2.1 Split source acquisition from audio writers in the recorder controller and mixer. Preserve existing recording and monitor-only behavior.
- [x] 2.2 Add a bounded per-source tap and local worker transport. Preserve original frame positions, discontinuities and consumer cutoffs.
- [x] 2.3 Seal recording and transcription independently. Test Stop both, fatal writer failures and slow workers against the unchanged recording path.
- [x] 2.4 Persist overlap references and recording frame maps where live outputs need them. Test pause, skipped silence, source restarts, codec delay and rollover.

## 3. Implement the worker and durable text

- [x] 3.1 Add live chunk scheduling, source-scoped confirmation and capability-aware multipart requests. Preserve CLI behavior and endpoint prefix handling.
- [x] 3.2 Add stable segment identity, conservative boundary deduplication, ordered commits, gap records and bounded overload handling.
- [x] 3.3 Add append-only recovery and a readable partial TXT. Test power-loss boundaries, access by another process, name collisions and unknown requests.
- [x] 3.4 Finalize on Stop transcription. Add bounded drain, cancellation, incomplete status, destination retry and cache cleanup after verified publication.
- [x] 3.5 Queue completed files and sessions using existing job locks and cache keys. Enforce global and per-destination concurrency limits. Test admission, live priority, cancellation and explicit resume.

## 4. Improve source selection and the native UI

- [x] 4.1 Add bounded recent-program history, explicit forget, optional automatic selection and safe matching after restarts or updates.
- [x] 4.2 Add independent recording and transcription controls, Stop both, live speaker toggle, output paths and queue status.
- [x] 4.3 Keep recording settings and speech settings in their respective sections. Add accessible action names, focus order and keyboard status.
- [x] 4.4 Run the narrow-window and DPI matrix. Verify the source picker, WAV sample rate and Opus bitrate controls with both activities running.
- [x] 4.5 Load and save portable transcription settings. Ship the example and check both fresh Windows ZIPs.
- [x] 4.6 Reuse the current transcription settings in guided batches. Keep both settings panels visible on request during capture.

## 5. Qualify optional processing

- [ ] 5.1 Add recognition-only speech gating with speech margins and recorded omission ranges. Compare storage, submitted audio, WER and quiet-word retention.
- [x] 5.2 Prototype a warm, bounded-window local speaker adapter. Test enable, disable, cold failure and teardown independently from transcription.
- [x] 5.3 Test provisional identity across overlapping windows. Preserve uncertain and simultaneous speakers. Test offline refinement against retained audio and approved export.
- [x] 5.4 Run paired offline/live and held-out speaker evaluations. Publish separate-track and mixed results with their scope and model footprint.

## 6. Verify and document delivery

- [ ] 6.1 Run deterministic CI with synthetic sources and a local mock. Check every published optional mode in the extracted Windows package.
- [ ] 6.2 Run the two-hour resource check and live device acceptance. Exercise a saved Discord/microphone fixture locally where available.
- [ ] 6.3 Update the operator guide, capabilities, benchmark charts and native UI images with verified results.
- [x] 6.4 Apply the published predecessor requirements before archiving the modified capabilities. Keep optional features off until their gates pass.

## Verification evidence

- In version `2.1.0-dev.2`, the extracted recorder accepted the packaged example and a personal config beside the executable. An invalid config preserved the panel values in a hidden Windows control test.
- In version `2.1.0-dev.3`, the extracted worker preferred the current panel values over a different sibling config. Declining the batch preserved the output folder. A hidden Windows test checked both panels with locked fields and a changing status area.

- The [maintainer guide](../../../docs/wiki/transcription.md) describes the local pipe, the source clock, the journal, and consent.
- The [live workflow](../../../docs/live-transcription.md) describes independent controls, recovery, batch admission, and optional speaker review.
- Compare [resource measurements](../../../docs/benchmarks/live-performance.md) and [quality measurements](../../../docs/benchmarks/live-quality.md) with the [acceptance criteria](acceptance.md).
- The four predecessor specifications preserve the 29 released requirements. A scratch archive of this change produced 11 canonical specifications. Strict validation passed on all 11.
- Testing ended at the operator's request. The joined recorder and worker ran for 4 minutes and 16 seconds. The two-hour gate remains open.
- The tested development ZIP grew 35.53% against the matched baseline. Two of five native-window peak memory comparisons exceeded the 2 MiB target.
- The saved call yielded recognized speech on both sources. A warmed local service completed 16 requests with zero coverage gaps.
- The extracted snapshot passed hardware and worker checks. The working tree contains later changes. Final package verification remains open.
- Review found a batch selection gap. Compare manifest outputs with separately selected files before approving a mix and its stems together.
