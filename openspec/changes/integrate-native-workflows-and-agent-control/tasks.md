## First implementation slice

The development build adds offline English transcription for one completed MP4. Local model import verifies the pinned weights, tokenizer, and configuration. The native window and agent adapters submit the same local job. The worker publishes plain text and timed JSON on the decoded audio clock.

Use the [operator guide](../../../rust/README.md#transcribe-an-mp4-offline) for model setup and the [Windows acceptance harness](../../../rust/tools/test_local_mp4.py) to repeat the three-client check. The state store and Windows file locks coordinate completed-file work. The shared capture owner and named-pipe protocol remain open for later slices.

### Windows package check

The 2026-11-24 local package check used one private English MP4 on a Ryzen 7 7800X3D with CPU Whisper `base.en`. Each client processed 348.648 seconds with the same pack and up to four inference threads. The three runs produced byte-identical text and JSON, with 1,094 words and 108 timed segments.

| Client | Elapsed | Verified behavior |
| --- | ---: | --- |
| Native UI | 118.83 s | Imported the pack, approved the job, and displayed two timed pages on an inactive Windows desktop. |
| Stdio MCP | 135.41 s | Disconnected after approval and retrieved the completed job from a new connection. |
| JSON CLI | 154.40 s | Received the approval receipt before inference completed and fetched every transcript page. |

The test observed zero visible console windows. It preserved the source checksum and left the interactive desktop unchanged. Peak committed memory for one process ranged from 569.9 to 570.6 MiB across the three runs. Each time represents one run under the available machine load.

Seeking to a recognized boundary reduced normalized word edits against the earlier endpoint transcript from 85 to 18 on this sample. Both transcripts came from recognizers. A human-reference accuracy trial remains open.

## 1. Freeze the contracts

- [ ] 1.1 Inventory existing controls, CLI verbs, cache schemas and profile versions. Map each to its UI and agent operation.
- [ ] 1.2 Define shared Rust commands, errors, events, output references, permission scopes and preview receipts. Add cross-client contract fixtures.
- [ ] 1.3 Rehearse predecessor archive order in a scratch tree. Preserve video, source-time, profile and guided-script compatibility tests.

## 2. Qualify the local runtime

- [ ] 2.1 Prototype native Rust English recognition with compatible weights on Windows x64. Measure recognition quality, real-time factor, timestamps, CPU and memory.
- [ ] 2.2 Benchmark multilingual recognition and quantized candidates separately. Record license, format, conversion provenance and digest for each pack.
- [ ] 2.3 Implement local model import, explicit download, validation, activation, removal and version pinning. Exercise failure, cancellation and offline startup.
- [ ] 2.4 Compare native VAD, segmentation and embeddings against held-out speaker and quiet-speech baselines. Keep unqualified capabilities disabled.

## 3. Deliver the first native workflow

- [ ] 3.1 Extract application services from GUI and console handlers. Connect a single session owner, worker scheduler and progress journal.
- [ ] 3.2 Implement local and compatible-endpoint profiles, import and save, explicit service checks and migration from version 1.
- [x] 3.3 Add native file selection, preview, approval, queue progress and timed transcript review for a completed MP4.
- [ ] 3.4 Replace built-in audio conversion and timing probes that invoke external media tools. Verify the supported codec matrix on a clean system.

## 4. Expose the same workflow to agents

- [ ] 4.1 Implement the noninteractive JSON CLI and stdio MCP adapters against the shared commands. Publish schemas and capability discovery.
- [ ] 4.2 Enforce OS identity, scoped grants, input binding, idempotency and expected revisions. Test same-user concurrent clients and rejected connections.
- [ ] 4.3 Verify source discovery, headless start and stop, leases, polling, cancellation, stale cursors, disconnect and restart.
- [x] 4.4 Run the first MP4 vertical slice through all three clients with identical settings and compare the resulting state and transcript.

## 5. Complete workflow parity

- [ ] 5.1 Integrate recording controls, independent live text, presets, recents, markers, portable settings and native device failure handling.
- [ ] 5.2 Integrate mixed batches, source resolution, retry, cancellation, partial results, recovery and offline refinement.
- [ ] 5.3 Implement speaker session preparation, attribution, evidence-based naming and correction review with native controls and agent actions.
- [ ] 5.4 Implement memory scope preview, export, import and withdrawal replay. Test permissions independently from transcript reads.
- [ ] 5.5 Run every operation in the design table without requiring a shell or browser. Retain existing explicit command-line entry points.

## 6. Qualify and document

- [ ] 6.1 Run the packaged [acceptance matrix](acceptance.md), resource comparisons and quality gates. Document exact profiles that pass.
- [ ] 6.2 Verify keyboard and DPI behavior in an isolated Windows session. Verify the complete GUI process tree produces no console popup.
- [ ] 6.3 Publish configuration examples, model notices, UI images, MCP client configuration, headless examples and measured charts. Keep private artifacts local.
- [ ] 6.4 Verify the signed or digest-pinned model artifacts, public package inventory, existing endpoint workflows and prior result readability before release.
