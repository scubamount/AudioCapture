# Acceptance plan

These are proposed qualification targets. The v2.0.0 measurements cover completed recordings and offline processing. Live accuracy, resource use and latency require new measurements. [Released measurements and limits](../../../docs/performance.md)

## Test conditions

Use release builds on the same Windows 11 x64 reference PC. Record CPU, RAM, storage, OS build, power plan, source format, model hashes, endpoint version and configuration. Pin the baseline to v2.0.0. Run five paired trials in alternating order. Report individual results, medians, p95, active audio duration, queue high-water marks and dropped frames. Use longer endurance runs for buffer growth and stop behavior.

Replay deterministic program and microphone PCM on a shared clock. Include quiet onsets, speech from one source during silence on the other, interruptions, repeated words, overlap, long quiet intervals and continuous speech. Use fixture injection and a controllable local multipart server in CI. Use actual Windows process capture for the release check. Keep private inputs and utterances out of public artifacts.

## Proposed budgets

| Measure | Pass condition |
| --- | --- |
| Recording only | Zero ASR requests, workers, model loads and live audio buffers. Retained frames and marker positions match v2.0.0 on the same PCM and settings. Median process CPU increase ≤1 percentage point of one logical core. Peak recorder working set increase ≤2 MiB. |
| Capture under load | Existing 480-frame blocks at 48 kHz span 10 ms. Enabling the live tap introduces zero extra recording drops. Count blocking I/O calls, waits for worker capacity and steady-state heap allocations on the capture thread. Each count must be zero for the tap. |
| Live audio buffering | Across all inputs in one session, the recorder tap holds ≤16 MiB. Worker ingress and ASR staging together hold ≤32 MiB of decoded PCM in RAM. Temporary ASR audio on disk stays ≤64 MiB by default. Include overlap and in-flight payload copies. Bound model workspace separately. |
| Live text latency | In the balanced test profile, cap core chunks at 4 seconds per source. Use two active sources and a loopback response time ≤1 second per request. After warm-up, p95 time from eligible segment end to readable committed text is ≤8 seconds. Report missing replies and gaps in the same sample. |
| Text durability and stop | Make committed text visible within 1 second of recognition commit. Persist a recoverable checkpoint at least every 2 seconds while commits arrive. Stop accepting new audio at the requested frame. Default pending-recognition drain ≤15 seconds. File copy and final rename can finish afterward. |
| Long calls | After ten minutes of warm-up, extend the same mix from 30 minutes to two hours. Steady-state private bytes for the recorder and conventional ASR worker grow ≤8 MiB each. Measure the optional model process separately. Transcript and journal size may grow on disk. |
| Local live speakers | On the reference PC, warm analysis of a two-source call takes ≤0.8 seconds per second of call time. Target p95 provisional label delay ≤15 seconds after eligible speech. Count unassigned and late turns. Report cold start. |
| Local model memory | Measure the complete speaker-process tree using the same model, host, source layout and sampling interval as the offline baseline. Its peak working set must stay within 125% of the measured offline peak plus 32 MiB. From 30 minutes to two hours, warm private bytes may grow by at most 32 MiB. |
| UI responsiveness | With a stalled endpoint and an active queue, p95 input-to-status acknowledgement ≤100 ms. Source fingerprinting, model startup and final file writes run off the UI thread. |
| Packaging | Recorder-only users install no model runtime or weights. From the same compiler and build flags, target a compressed ZIP ≤110% of the v2.0.0 rebuild. Report the absolute size and delta. |

Treat the capacity and drain limits as enforced defaults. Thresholds for quality and performance are release gates on the reference configuration. Report failed targets before qualifying a profile. A custom endpoint can have a different response time. Display its observed lag or coverage loss. Include slow, unknown-outcome and offline cases even when the low-latency target excludes them.

## Correctness and storage

1. Exercise every transition in the [control table](specs/live-capture-session/spec.md). Test cancellation, rapid repeated commands, different stop orders and a new interval while a prior result finalizes.
2. Use recording [5, 12) inside transcription [0, 20). Reverse the order in another call. Verify no invented audio offsets and one final file per interval.
3. Compare regular Opus and speech DTX at 32 kbps mono on the same decoded samples. Vary recording silence removal and recognition gating independently. Keep mono, bitrate, thresholds and all other settings fixed for each paired comparison.
4. Report container bytes, saved frames, seconds submitted, transferred bytes, request count and final TXT bytes separately. Show onset and tail errors, word deletions, insertions and substitutions against human reference text. Never derive transcript quality from byte counts.
5. Use a fixture with at least 50% confirmed silence. For the candidate gated profile, require ≥25% fewer submitted audio seconds and no lost annotated quiet boundary words. Include an all-speech control. Require absolute word error rate increase ≤1 percentage point against the ungated run on the held-out speech set. Withhold the gated profile from release if it misses qualification.
6. Resolve first and last retained frames around pause, silence removal, rollover and source loss. Verify exact frame mapping on PCM and report codec delay separately. Validate seeking, marker positions and simultaneous speakers.
7. Run a 100-item batch and start live while one request stalls. Verify the visible wait, absolute 120-second default deadline, actual live start frame and subsequent batch resumption. Test spare slots separately. Delay, reorder and repeat responses. Crash after sending a request. Verify bounded resources, honest gaps, repeated words and zero automatic resubmission of unknown outcomes.
8. Interrupt each cache and publication step, including a torn UTF-8 tail, file collision, low disk space and destination loss. Reopen twice with the network disabled. Confirm identical recovered text and stable segment IDs. A zero-speech interval has an identified empty outcome.
9. Read the partial TXT in another process while words arrive. Confirm the file grows incrementally. Repeat start/stop within one second. Verify neither interval overwrites its predecessor.
10. Run recording alone under worker and network tracing. Stop recording during a live transcript and inspect the file handles. Stop transcription during recording and inspect network traffic, workers and remaining audio.

## Speaker quality

Compare live windows and the released whole-source adapter on identical inputs with the same scoring rules. Run the 24-call synthetic regression set. Freeze separate held-out scripts, speaker combinations and overlap patterns before tuning. Score mixed and separate tracks independently. Use reference word times for the diarizer test and real ASR output for the end-to-end test.

Report diarization error rate, isolated-turn recall, overlap error, unknown coverage, speaker ID switches and label delay. Count abstentions in recall. The initial qualification target on separate tracks is ≤20% diarization error and ≥90% isolated-turn recall. On the held-out paired set, require live error within 3 percentage points of the offline baseline. Publish mixed-source results separately. Passing the separate-track gate qualifies only that layout.

Verify disabling labels releases the model after bounded shutdown and leaves transcription active. Check that final refinement preserves every raw word, handles missing audio and never promotes a provisional name to a durable identity. Use held-out human calls with reference turns before making claims about natural conversation.

## Interface and release evidence

Exercise normal, narrow and high-DPI layouts at 390 and 660 logical pixels and 96, 120, 144 and 192 DPI. Use the keyboard and Windows accessibility inspection. Verify the two primary actions, **Stop both**, **Transcribe files…**, source selector, live status, WAV rate and Opus bitrate. Check focus and readable status during startup, lag, disk failure and independent stop. Include a native screenshot at narrow width.

Publish aggregate metrics, tested commit, pinned fixture inputs and commands to reproduce. Keep the recording-only, mock-endpoint, live device, and local-model results clearly identified. Promote a profile only after its own checks pass.
