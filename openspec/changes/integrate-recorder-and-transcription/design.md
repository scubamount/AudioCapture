# Design

## Native entry point

Enable **Transcribe files…** when the recorder is idle. Open a native multi-select picker for supported audio files. Canceling the picker returns to the recorder. After selection, resolve `transcribe.exe` beside the running recorder and launch it in a new console. Pass `--guided --` and each selected path as a separate argument. Use Windows argument quoting for spaces, Unicode, and option-like file names. Let Windows attach the worker's input and output to its console even when the recorder's streams are redirected. Report a missing or failed companion launch in the recorder. Keep the recorder responsive while the console is open.

Only this explicit action launches the worker from the recorder. Starting, pausing, marking, or stopping capture makes no transcription request. The native process does not handle endpoint settings or submit audio.

## Guided worker

Prompt for the endpoint base URL, model, output folder, and optional API-key environment variable name. Default the output folder to `transcripts` beside the first selected file. Prompt for a shared session, timestamps, long-file chunking, and HTML review. Default to separate timelines, timestamps off, and chunking off. Enable review by default and choose a fresh review filename. Reuse the standalone worker's validation, job processing, and output handling.

Before submission, show the selected files, endpoint, model, output folder, and effective options. Keep credentials out of the summary. Default the final prompt to no. Blank input, no, or end of input cancels before the worker sends audio. Closing the prompt before confirmation has the same result. The existing scripted command retains its invocation and behavior.

## Local processing

Check supported file metadata and the configured byte limit before hashing, opening cached results, or probing duration when chunking is off. Report an oversized item and continue eligible items. Keep the opted-in chunking and restart paths. Use buffered reads to load cached JSON through the existing parser. Preserve raw responses, job outcomes, and review timing.

Use the valid frame count directly for recorder blocks where every frame is kept. Avoid building or scanning a per-frame keep mask for these blocks. Retain the filtered path for pause or silence removal. Map markers with the same retained-frame count the audio writer uses. Compare decoded PCM, frame counts, and marker sidecars against the existing behavior.

## Portable delivery

Package the recorder, its sibling worker, and the portable documentation and notices. Keep model weights and optional FFmpeg tools external to the package. Document the picker and confirmation flow first. Document scripted use, manual endpoint setup, and optional tools beside that flow. Retain the legacy C++ build and package its dedicated guide.
