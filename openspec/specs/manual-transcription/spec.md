# manual-transcription Specification

## Purpose

Transcribe approved, completed recordings using a companion worker and a compatible endpoint. Keep the results available for local review.

## Requirements

### Requirement: Explicit transcription requests

The companion command SHALL transcribe one or more selected completed audio files only after an explicit user invocation.

#### Scenario: Record audio

- **WHEN** a user records, stops, or opens audio without invoking transcription
- **THEN** the recorder makes no transcription request

#### Scenario: Submit a batch

- **WHEN** a user invokes the command with several files and a configured model and endpoint
- **THEN** each selected item has an independent result or reported failure

### Requirement: Custom transcription endpoint

The companion SHALL support a user-supplied compatible base URL, model, and optional explicit authentication. It SHALL stream multipart audio to the configured transcription route.

#### Scenario: Prefixed endpoint

- **WHEN** the configured base URL ends in `/team/v1/`
- **THEN** the request targets `/team/v1/audio/transcriptions`
- **AND** includes the selected file and model

#### Scenario: Redirect response

- **WHEN** the endpoint responds with a redirect
- **THEN** the client reports the response without forwarding the audio or key

#### Scenario: Endpoint response

- **WHEN** the endpoint returns supported plain text or JSON containing text
- **THEN** the client saves the transcript and retains the raw JSON when supplied

### Requirement: Safe batch progress

The companion SHALL preserve source files and completed results. It SHALL distinguish known failures from an unknown remote outcome.

#### Scenario: Timeout after upload

- **WHEN** a request times out after the provider may have received the audio
- **THEN** the item is recorded as unknown and is not automatically resent
- **AND** the user may explicitly retry it

#### Scenario: Existing output or duplicate basename

- **WHEN** an output exists or selected files have the same basename
- **THEN** each result keeps an unambiguous identity without overwriting existing data

#### Scenario: Oversized input

- **WHEN** a file exceeds the selected endpoint limit and no validated chunking mode is available
- **THEN** the client reports that item without uploading a truncated file
- **AND** preserves successful results from other items

#### Scenario: User enables long-file chunking

- **WHEN** the user selects long-file chunking and an installed FFmpeg can process the oversized source
- **THEN** the client submits bounded 16 kHz mono PCM chunks one at a time with known source offsets
- **AND** keeps original audio, per-chunk raw results, and durable outcomes

#### Scenario: A chunk fails after other chunks complete

- **WHEN** a chunk fails or has an unknown remote outcome
- **THEN** the client reports the affected chunk and keeps successful chunks and other batch results
- **AND** an unknown remote outcome requires explicit retry

#### Scenario: A long session exceeds memory

- **WHEN** a synthetic two-hour source is chunked
- **THEN** processing uses bounded memory and temporary disk space rather than loading the entire source into memory
- **AND** the result records the resource measurements and source offsets

### Requirement: Source-labeled transcript review

The companion SHALL preserve source labels, raw provider text, and valid available recording-relative times. It SHALL offer local search, audio replay for timed segments, and a separate correction export.

#### Scenario: Concurrent source segments

- **WHEN** Discord and microphone responses contain overlapping valid segments
- **THEN** review retains both segments with their source labels and times

#### Scenario: Unrelated sessions in one batch

- **WHEN** a batch contains files from separate or unidentified sessions
- **THEN** review keeps their timelines separate rather than assigning a shared origin

#### Scenario: Untimed or invalid response

- **WHEN** a provider omits timestamps or returns values inconsistent with the source duration
- **THEN** the affected text remains available without fabricated timing

#### Scenario: Review a transcript

- **WHEN** a user searches, edits, or exports a transcript
- **THEN** the raw provider result remains unchanged
- **AND** local review does not initiate a new transcription request

### Requirement: Native manual entry point

The Windows recorder SHALL offer **Transcribe files…** only while idle. It SHALL open a native picker for multiple supported audio files and launch its sibling worker in a separate console after selection.

#### Scenario: Select completed tracks

- **WHEN** an idle user selects files in the native picker
- **THEN** the recorder launches its sibling `transcribe.exe` with `--guided --` and one argument per selected path
- **AND** spaces, Unicode, and option-like names reach the worker intact
- **AND** the recorder remains responsive

#### Scenario: Redirect the recorder's standard streams

- **WHEN** the recorder starts with redirected standard streams and the user launches transcription
- **THEN** the worker displays prompts and accepts input in its new console

#### Scenario: Cancel selection or fail to launch

- **WHEN** the user cancels the picker or the companion cannot start
- **THEN** no transcription request occurs
- **AND** the recorder reports a launch failure when one occurs

#### Scenario: Capture without transcription

- **WHEN** the user starts, pauses, marks, or stops a recording
- **THEN** the recorder makes zero transcription requests
- **AND** the action is unavailable during active capture

### Requirement: Guided settings and consent

The guided worker SHALL collect a base URL, model, output folder, and optional API-key environment variable name. It SHALL offer shared-session alignment, timestamps, chunking, and HTML review. It SHALL show the files, destination, and effective settings before asking for affirmative upload consent.

#### Scenario: Use guided defaults

- **WHEN** the user accepts the optional defaults
- **THEN** output targets a `transcripts` folder beside the first selected file
- **AND** tracks use separate timelines with timestamps and long-file chunking off
- **AND** the worker enables HTML review with a fresh output filename

#### Scenario: Consent to upload

- **WHEN** the user confirms the final submission prompt
- **THEN** the worker may upload the confirmed files with the shown settings
- **AND** the summary and output exclude the API key

#### Scenario: Decline or interrupt confirmation

- **WHEN** the user enters no, leaves the final answer blank, or reaches end of input before confirming
- **THEN** the worker sends zero transcription requests

#### Scenario: Use the scripted command

- **WHEN** the user invokes the worker with the existing scripted arguments
- **THEN** the command runs without guided prompts and retains its existing options and output behavior

### Requirement: Efficient local processing with equivalent results

The worker SHALL use file metadata to reject oversized items early when chunking is off. It SHALL read cached JSON through a buffer and preserve the existing results and recovery behavior.

#### Scenario: Reject oversized input before content work

- **WHEN** a file exceeds the configured limit and chunking is off
- **THEN** the worker reports the per-file failure before hashing or duration probing
- **AND** the file causes zero uploads and processing continues for eligible items

#### Scenario: Resume with a cached JSON result

- **WHEN** a completed result is reused in the same batch configuration
- **THEN** cached JSON loads through buffered input and creates no duplicate upload
- **AND** the raw response, transcript, and timing remain equivalent

#### Scenario: Render a block that keeps all its frames

- **WHEN** a recorder block retains every valid frame, including in a partial final block
- **THEN** the recorder skips building and scanning a per-frame keep mask
- **AND** the decoded PCM and retained frame count equal the reference result
- **AND** markers use the same output timeline across kept and removed frames

### Requirement: Portable recorder and worker delivery

The Windows archive SHALL include the two executables as siblings with their documentation and dependency notices. It SHALL keep optional model and external media tools outside the archive. The repository SHALL retain the legacy C++ recorder and its build path. The legacy package SHALL include its dedicated guide.

#### Scenario: Extract and run the portable package

- **WHEN** a user extracts the Windows archive and selects completed recordings through the native action
- **THEN** the recorder finds the bundled worker without a PATH dependency
- **AND** the package documentation explains guided use, scripted use, and optional tool prerequisites
