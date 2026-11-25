## Why

A Discord recording can contain several remote speakers on one track. The present worker identifies the track and retains recognition results. An agent also needs complete text, source references, and a way to apply later corrections.

## What Changes

- Add an optional worker step for speaker turns in selected, completed sessions. Process existing recognition artifacts offline with an explicitly installed local adapter.
- Use evidence from selected conversations to propose names. Show the evidence and support local correction, confirmation, and withdrawal.
- Add a local, machine-readable session transcript and a versioned export stream. Consumers can import complete conversations incrementally and retract corrected or withdrawn material.

This proposal specifies future work. The recorder, worker, and portable package do not implement these capabilities through this change.

## Capabilities

### New Capabilities

- `speaker-attribution` covers track selection, diarization, timing, contextual naming, and review.
- `transcript-memory-export` covers complete session transcripts, reviewed export scope, stable references, and incremental import.

### Modified Capabilities

None.

## Impact

Implementation would extend the companion worker and local review. It would consume existing transcription results and keep original artifacts intact. The recorder would retain its current manual entry point.
