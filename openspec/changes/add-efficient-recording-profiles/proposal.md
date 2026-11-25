# Add efficient recording profiles

## Why

Speech recordings can take less space when the recorder uses mono Opus. Users currently have to set the format, channel count, and bitrate one at a time.

Named presets store the entire capture setup. Users also need a way to change the encoding while keeping the sources and routes they already configured.

## What Changes

- Offer six built-in recording profiles in the Windows recorder.
- Start new installations with regular 32 kbps mono Opus, based on a controlled offline comparison.
- Show the profile that matches the current encoding or show **Custom**.
- Keep named saved setups and the existing manual encoding controls available.
- Document the profiles, the measurement limits, and the local speech test procedure.

## Capabilities

### New Capabilities

- `recorder-documentation` covers profile guidance, a source-linked wiki, and privacy-safe evaluation evidence.

### Modified Capabilities

- `recorder-preferences` adds encoding profiles, first-run behavior, and compatibility requirements alongside the open Rust rewrite.

## Impact

The Rust preference loader and Windows interface gain encoding profiles. The manual format controls and the existing recording, routing, and transcription paths remain available. Private recordings and detailed transcript comparisons stay in local evaluation output.
