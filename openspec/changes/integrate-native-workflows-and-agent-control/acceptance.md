# Acceptance plan

These are proposed release gates. The [first implementation slice](tasks.md#first-implementation-slice) records the completed-file checks. Full release qualification includes the separate live, speaker, and endurance gates below.

## End-to-end checks

| Gate | Exercise | Required evidence |
| --- | --- | --- |
| Original failure | On a fresh Windows extraction, select a synthetic spoken MP4 and local recognition. Use an empty PATH, networking disabled, and no listening ASR server. | Import a compatible model through the product. One approval yields nonempty text, timed segments and native review. Log the model digest and output hashes. |
| No console or desktop automation | Execute every built-in UI workflow with stdin closed. Trace process creation and window visibility for the whole child tree. | Zero visible console windows, shell scripts, input prompts, Python, or external codec executables. Verify focus and cancellation. |
| UI, CLI and MCP parity | Run the design coverage table against isolated copies of the same fixtures. Use the MCP handshake and a real client. Run the CLI with closed stdin. | Same capabilities, state transitions, errors, artifacts, time offsets and revisions. Normalize generated IDs before comparison. No GUI is created by headless commands. |
| Shared owner and permissions | Attach GUI and two clients, replay a commit, use a stale plan, exceed an allowed scope and disconnect clients in either order. | One active capture and one submitted job. Denied requests cause no effects. Headless leases and approved batch jobs follow the documented lifecycle. |
| Files and clocks | Mix a finished recording timeline, multiple sources, a delayed track, multi-track MP4, fragmented MP4, video-only file and long audio. | Preserve intervals and first-track selection. Keep per-file failures isolated. Send zero video bytes. Unrelated sources keep independent clocks. |
| Local versus endpoint | Import a v1 loopback profile. Exercise a custom URL prefix, a closed port, HTTP 404 and a lost connection after submission. | Exact destination is visible. Unsent work can retry normally. Unknown outcomes need explicit retry. Local jobs never upload or silently switch provider. |
| Two live consumers | Start and stop recording and text in both orders. Simulate a slow worker, device loss, model failure, session restart and full queue. | Surviving consumer stays active. Final files and recoverable text reconcile to the source clock. Gaps and cancellation boundaries are visible. |
| Speakers and memory | Use separate sources, a mixed conversation, overlap, an unsupported name inference, a corrected name, and a withdrawal. | Same revision and evidence on all clients. Uncertain identities stay uncertain. Export only the approved scope. Repeat import is idempotent. |
| Privacy and package | Probe status before approving input. Test expired grants, changed inputs, revoked access and failed model installation. Inspect the public ZIP. | No private audio, transcript, credentials or absolute private paths in default logs, discovery, public examples or model packs. Partial installs never replace an active model. |

CI uses generated audio, a local multipart fixture and headless control. Run visible-window, keyboard, screen-reader, 125–200% DPI and narrow-window checks in a disposable Windows session. Verify a real program and mic in a separately approved device trial. Publish only aggregate figures for private tests. A browser is optional for exported HTML review.

## Performance and quality

Record baseline and candidate commits, build flags, model hashes, host, thread limits and emitted audio frames. Separate executable size, downloaded weights, active inference memory, and recorder memory. Use five paired runs on the same Windows 11 x64 reference PC. Keep raw samples with medians and p95. Reuse the [capture budgets](../add-independent-live-transcription/acceptance.md) and [published v2.1.0 endurance method](../../../docs/benchmarks/live-performance.md).

| Measure | Proposed decision rule |
| --- | --- |
| Recording only | Zero model loads, recognition traffic and extra dropped frames. Target at most 2 MiB more peak recorder working set and one percentage point more CPU on one core versus the matched baseline. |
| Endurance | Run the ten-minute Windows release check. Allow at most 8 MiB growth in warm private bytes for the recorder and conventional worker. Report model memory separately. Run the two-hour model and capture check before claiming long-session qualification. |
| Live processing | For the designated CPU live profile, process both paced sources with total warm compute below one second per second of session audio. Target p95 readable-text delay of eight seconds. Report gaps, quiet misses and backlog. Reject a live profile that meets speed by dropping speech. |
| Bounded work | Exercise endpoint stall, client event lag and overload. Enforce existing PCM and scratch limits. Bounded queues return a gap or an actionable busy condition. Report model workspace separately. |
| Recognition quality | Compare to local faster-whisper `base.en` on held-out English speech with human reference text. Proposed gate: word error rate within two absolute percentage points, with quiet-mic and program scores reported separately. Test each advertised language on its own corpus. |
| Speaker quality | Apply the prior held-out overlap, missed-turn, false-label and delay tests. Keep failing modes unavailable for production on all surfaces. Record source-only labels separately from speaker predictions. |
| Footprint and latency | Measure compressed core ZIP, optional runtime and each model pack separately. Report cold start, unload latency, p95 UI interaction and status-query latency under inference. Compare optional libraries against a build without inference. |

Repeat the spoken MP4 check using the packaged worker and with a real offline model. A mock endpoint can qualify transport and scheduling only. Publish method, build, limits, and failing gates before enabling a default. [Prior speech measurements](../../../docs/benchmarks/live-quality.md)
