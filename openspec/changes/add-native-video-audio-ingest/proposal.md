## Why

The completed-file picker accepts MP4 files. Version 2.1.0 can send the entire video to recognition, although the service needs only audio.

## What Changes

- Extract the first embedded audio track locally when a user submits a supported video.
- Use the same guided batch, scripted command, transcription config, and result files as existing audio jobs.
- Limit each upload by decoded audio size. Show a file error if Windows cannot read its audio.

## Capabilities

### Modified Capabilities

- `manual-transcription` adds native video input to completed-file transcription.

## Impact

The Rust companion uses Windows Media Foundation on demand. The recorder adds a video filter to its existing file picker.
