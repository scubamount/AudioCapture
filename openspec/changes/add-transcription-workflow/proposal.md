# Add on-demand transcription

## Why

The recorder produces source-isolated files for Discord and microphone audio. Users need to request transcripts for selected recordings and use their chosen OpenAI-compatible transcription endpoint. A source-labeled review makes it easier to check a model's words against the audio.

## What Changes

- Add a standalone Rust transcription command for selected finalized files and explicit batch requests.
- Allow a custom endpoint base URL, model identifier, optional key, and supported response settings.
- Preserve original inputs and raw provider responses. Report per-file state, errors, and retry ambiguity.
- Provide a local transcript review that retains source labels and available timestamps.
- Evaluate speech detection and model choices with the existing quality spike.

## Capabilities

### New Capabilities

- `manual-transcription` covers explicit requests, custom endpoint transport, batch persistence, source timing, and local review.

### Modified Capabilities

The recorder follow-up retains its separate file modes and marker support. The quality spike remains a separate evaluation.

## Impact

The Windows portable build gains a separately invoked transcription command. The recorder remains usable without a configured endpoint or an installed model. Provider compatibility and model quality require the documented tests.
