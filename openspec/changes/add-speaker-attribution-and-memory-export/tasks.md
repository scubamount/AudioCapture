## 1. Inputs and local processing

- [ ] 1.1 Define the session manifest, stable ID registry, schema versions, and fixture generator.
- [ ] 1.2 Add explicit guided and batch session selection with canonical track and chunk validation.
- [ ] 1.3 Load current job, text, provider JSON, and aggregate artifacts without changing originals.
- [ ] 1.4 Implement the versioned local diarization adapter and per-source failure handling.
- [ ] 1.5 Normalize known provider capabilities and preserve full text for untimed or unsupported results.
- [ ] 1.6 Align bounded intervals and handle overlaps, missing data, chunk offsets, and unrelated clocks.

## 2. Evidence and review

- [ ] 2.1 Generate session-local speaker labels and traceable contextual naming suggestions.
- [ ] 2.2 Add local review for attribution, optional inferred labels, confirmation, and identity correction.
- [ ] 2.3 Record cross-session link decisions and invalidate inferences when cited evidence changes.

## 3. Transcript and export

- [ ] 3.1 Serialize complete versioned transcripts with raw text spans, provenance, coverage, and stable IDs.
- [ ] 3.2 Add local export preview with exclusions, redactions, and an explicit provisional-name policy.
- [ ] 3.3 Publish atomic snapshot and delta batches with checkpoints, replacement records, and tombstones.
- [ ] 3.4 Test a reference importer for stale events, retries, splits, merges, and downstream invalidation.

## 4. Acceptance and delivery

- [ ] 4.1 Run offline fixtures with networking unavailable and record exit status and output coverage.
- [ ] 4.2 Score the held-out synthetic corpus against the stated timing, diarization, and naming gates.
- [ ] 4.3 Verify full text reconstruction, privacy filtering, correction propagation, and replay equivalence.
- [ ] 4.4 Verify legacy worker invocations and zero recorder network activity during capture and browsing.
- [ ] 4.5 Document actual commands, supported adapters, evidence limits, and import responsibilities after implementation.
