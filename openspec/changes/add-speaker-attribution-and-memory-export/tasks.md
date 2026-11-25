## 1. Inputs and local processing

- [x] 1.1 Define the session manifest, stable ID registry, schema versions, and fixture generator.
- [x] 1.2 Add explicit guided and batch session selection with canonical track and chunk validation.
- [x] 1.3 Load current job, text, provider JSON, and aggregate artifacts without changing originals.
- [x] 1.4 Implement the versioned local diarization adapter and per-source failure handling.
- [x] 1.5 Normalize known provider capabilities and preserve full text for untimed or unsupported results.
- [x] 1.6 Align bounded intervals and handle overlaps, missing data, chunk offsets, and unrelated clocks.

## 2. Evidence and review

- [x] 2.1 Generate session-local speaker labels and traceable contextual naming suggestions.
- [x] 2.2 Add local review for attribution, optional inferred labels, confirmation, and identity correction.
- [x] 2.3 Record cross-session link decisions and invalidate inferences when cited evidence changes.

## 3. Transcript and export

- [x] 3.1 Serialize complete versioned transcripts with raw text spans, provenance, coverage, and stable IDs.
- [x] 3.2 Add local export preview with exclusions, redactions, and an explicit provisional-name policy.
- [x] 3.3 Publish atomic snapshot and delta batches with checkpoints, replacement records, and tombstones.
- [x] 3.4 Test a reference importer for stale events, retries, splits, merges, and downstream invalidation.

## 4. Acceptance and delivery

- [x] 4.1 Run offline fixtures with networking unavailable and record exit status and output coverage.
- [x] 4.2 Score the held-out synthetic corpus against the stated timing, diarization, and naming gates.
- [x] 4.3 Verify full text reconstruction, privacy filtering, correction propagation, and replay equivalence.
- [x] 4.4 Verify legacy worker invocations and zero recorder network activity during capture and browsing.
- [x] 4.5 Document actual commands, supported adapters, evidence limits, and import responsibilities after implementation.


## Implementation and verification

The companion worker now prepares, attributes, and reviews completed sessions on request. It can preview and publish local JSONL batches. Its reference importer applies corrections, replays, and withdrawals. The operator guide covers the command and configuration contracts. [Speaker workflow](../../../docs/speaker-workflow.md)

Tests reconstructed the canonical transcript bytes and exercised mixed and separate inputs, unknown timing, speaker overlap, stale evidence, scoped exports, and interrupted writes. The local Windows test used both original recordings from a single call and kept the audio and exact transcript private.

Diarization quality depends on the recording layout. Both tested backends met the acoustic targets on separate program and microphone tracks. Both missed the targets on the single mixed recording. The performance report lists the measured scope and results. [Performance](../../../docs/performance.md)

The packaged worker ran inside a Windows AppContainer with zero network capabilities. IPv4 and IPv6 loopback checks timed out there and succeeded outside. The real local adapter retained all 376 reference text bytes on a synthetic input. Reviewed export and replay succeeded with networking blocked.

With separate tracks, the acoustic backend supported 15 of 23 introductions and 15 of 21 contextual names. All 90 automatically named turns matched their reference speaker. Mixed local acoustic results retained suggestions only. The single-mix acoustic accuracy target remains unmet.

The legacy command and cache checks passed. Windows event tracing observed no recorder WinHTTP or Winsock activity during synthetic process capture or either native picker. Positive controls produced events. The packaged recorder matched the picker-tested build after removing linker timestamps and the debug identifier.
