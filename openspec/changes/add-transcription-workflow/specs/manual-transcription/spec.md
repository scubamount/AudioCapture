## ADDED Requirements

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
