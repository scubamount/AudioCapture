# Improve recorder reliability and efficiency

## Why

The recorder can lose buffered Opus audio during an abrupt exit. A late packet can stop an entire session. Users need a visible signal check before they rely on a transcript.

The review of commit `ce7089f` also found unnecessary packet allocations, ambiguous overlap in mixes, and defects in WAV output and legacy presets.

## What Changes

- Checkpoint Opus audio and bound the audio lost after a process interruption.
- Keep healthy sources recording when a source delivers audio after the committed timeline.
- Show source activity and prevent known feedback and duplicate-source mixes.
- Reduce packet allocation and pause-history work, with reproducible measurements.
- Preserve WAV filter tails and imported gains. Keep process discovery usable during partial endpoint failures.
- Bind release packages to the source and dependency state used to build them.
- Keep compact speech settings and separate-track output easy to select.

## Capabilities

### New Capabilities

- `recorder-resilience` defines interruption recovery, late-source handling, source activity, safe routing, output correctness, and measured resource use.

### Modified Capabilities

The existing Rust rewrite change remains open. This change adds follow-up requirements alongside that change.

## Impact

The changes affect the Rust recorder, its tests, the Windows UI, and the packaging helper. Existing recording formats, presets, and file modes remain supported. The transcript evaluation is a separate change.
