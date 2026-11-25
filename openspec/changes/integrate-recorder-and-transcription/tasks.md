# Implementation tasks

## 1. Native workflow and consent

- [x] 1.1 Add the idle action, supported-file picker, and sibling launch with each path passed as a distinct argument.
- [x] 1.2 Test the active-session command gate. Verify picker cancellation and missing-worker handling on Windows.
- [x] 1.3 Implement the guided prompts, defaults, settings summary, and affirmative final confirmation.
- [x] 1.4 Exercise native selection and the separate console with a synthetic recording. Verify the worker receives the selected path.
- [x] 1.5 Assert zero mock requests for a blank answer, no, and end of input. Verify the original command still runs without prompts.
- [x] 1.6 Confirm a guided batch against a loopback mock. Verify only its selected media reach the endpoint.

## 2. Local processing and recorder output

- [x] 2.1 Verify oversized rejection precedes hashing and duration probing with chunking disabled. Confirm the rest of the batch continues.
- [x] 2.2 Compare cached JSON read counts and verify identical parsed results. Confirm resume causes zero repeat uploads.
- [x] 2.3 Compare all-keep PCM, retained frame counts, and marker positions with the reference. Cover a partial final block and a block that removes frames.
- [x] 2.4 Verify the all-keep path avoids constructing or scanning a per-frame keep mask. Time the same workload before and after the change.

## 3. Portable delivery

- [x] 3.1 Build and inspect the locked Windows archive. Verify the sibling layout, documentation, and dependency notices.
- [x] 3.2 Verify the extracted recorder can launch its worker. Check the archive size and exercise the documented scripted command.
- [x] 3.3 Check legacy build inputs and run both packaging steps with an existing C++ executable. Verify each archive includes the dedicated guide.
- [x] 3.4 Validate this OpenSpec change in strict mode.

## Verification

Eight native tests and two worker launch tests covered the command gate, layout, arguments, and launch behavior. Four guided tests covered the prompts and consent summary. The packaged worker made zero loopback connections for blank, no, and end of input. It also canceled on an unterminated affirmative answer.

A Windows launch fixture verified that the worker gets interactive console handles under five parent launch and redirection modes.

On the reviewed package, picker cancellation started no worker. Selecting a synthetic WAV started the sibling with the intended path and an interactive console. The desktop run stopped at the endpoint prompt. The active-session gate passed its native test. The initial package showed the missing-worker error dialog, and the revised launcher passed a separate missing-worker check.

The confirmed loopback batch submitted both selected synthetic files and excluded the unselected fixture. The scripted command submitted one file without an interactive prompt. Reusing both saved results made zero repeat requests. Buffering reduced reads from 12,327 to at most three for the cached JSON fixture.

The size regression returned the expected limit error with the first content byte locked. A non-chunked mixed-size batch rejected the oversized input and submitted only the eligible source. A two-hour synthetic source sent nine chunks to a loopback mock. Peak working memory was 10.39 MiB for the worker and 23.71 MiB for FFmpeg. The repeat sent zero requests.

Eighteen recorder tests covered PCM, partial blocks, pauses, removed frames, and markers. Across 25 paired runs of 10,000 blocks, median processing time decreased from 14.629 ms to 13.434 ms.

The reviewed Rust archive measured 1,341,836 bytes. It contained both executables, the guide, an inventory of 40 dependencies, and 78 referenced notice files. The local run passed 95 focused Rust tests, four packaging tests, and five strict OpenSpec validations.

Both legacy packaging steps included a byte-for-byte copy of the dedicated guide. They used a prebuilt executable. A fresh C++ compile was not checked.

The packaged worker produced source-labeled transcripts from an existing Discord and microphone recording. A new live call and a hosted CI run were not checked.
