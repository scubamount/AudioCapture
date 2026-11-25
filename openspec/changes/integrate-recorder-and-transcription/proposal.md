# Integrate recorder and transcription

## Why

Users can record source-isolated audio in the native application. They currently need a separate command and its options to transcribe the saved files. Give them one entry point with explicit control over where their audio goes.

## What Changes

- Add an idle **Transcribe files…** action. Select completed files in a native picker and open the companion in its own console.
- Collect endpoint and review settings in the console. Show the planned submission and require affirmative confirmation before any upload.
- Reject oversized files before hashing when chunking is off. Buffer reads of cached JSON.
- Reduce routine recorder block work while preserving PCM and marker positions.
- Package and document the two executables together. Preserve the scripted transcription command and legacy recorder.

## Capabilities

### Modified Capabilities

- `manual-transcription` gains a native entry point, a guided consent flow, bounded local processing, and portable delivery requirements. Its standalone requirements are in `add-transcription-workflow`.

## Impact

The Rust recorder, transcription worker, and portable Windows package change. Capture continues to write locally. The companion uses the endpoint that the user selects.
