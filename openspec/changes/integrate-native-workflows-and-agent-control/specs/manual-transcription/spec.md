## MODIFIED Requirements

### Requirement: Native manual entry point

The recorder SHALL offer **Transcribe files…** while idle, recording, transcribing live, or finalizing an interval. It SHALL accept completed audio, video, and finalized recording sessions through a native multi-file picker. Selection, progress, and approval SHALL stay in the application.

#### Scenario: Select completed tracks

- **WHEN** an operator selects finished files
- **THEN** the native form lists each selected file, including names with spaces, Unicode, or leading option characters
- **AND** the operator can choose stems, mix, or explicitly acknowledged duplicate coverage for a finalized session
- **AND** active capture continues while the operator reviews the selection

#### Scenario: Redirect the recorder's standard streams

- **WHEN** the recorder runs with redirected or unavailable standard streams
- **THEN** the native workflow accepts input, displays results, and completes without a console

#### Scenario: Cancel selection or fail to launch

- **WHEN** the operator cancels the selection or the worker cannot start
- **THEN** the selected action sends no transcription request
- **AND** the UI reports a worker startup failure without interrupting other activities

#### Scenario: Capture without transcription

- **WHEN** the operator records, pauses, marks, or stops without selecting transcription
- **THEN** recording alone starts no recognition work

#### Scenario: Select the active recording

- **WHEN** a selected file is still open for recording
- **THEN** the application identifies the unfinished item and excludes it until finalized and approved
- **AND** selecting it does not start live transcription

### Requirement: Guided settings and consent

The application SHALL allow local recognition or an explicitly configured compatible endpoint. The form SHALL show the selected files, destination, effective model and settings before approval. It SHALL offer output location, timestamps, shared-session alignment, splitting for oversized audio files, and optional HTML review. Endpoint mode SHALL accept an optional API key environment variable name.

#### Scenario: Use guided defaults

- **WHEN** the operator accepts the existing guided defaults without a saved config or an output override
- **THEN** results go to a `transcripts` folder beside the first selected file
- **AND** tracks use separate timelines with timestamps and optional audio-file splitting off
- **AND** HTML review uses a fresh output filename

#### Scenario: Consent to upload

- **WHEN** the operator approves the displayed endpoint submission
- **THEN** the worker may send only the approved sources' audio under the displayed settings
- **AND** the summary and results omit credential values

#### Scenario: Decline or interrupt confirmation

- **WHEN** the operator declines, closes the form, or ends a guided command before confirming
- **THEN** the worker sends zero recognition requests

#### Scenario: Use the scripted command

- **WHEN** the operator invokes the existing scripted command with its established arguments
- **THEN** it retains its noninteractive behavior, options, and compatible result formats
- **AND** invoking the legacy guided command explicitly remains possible

### Requirement: Safe batch progress

The worker SHALL preserve sources and completed results. It SHALL distinguish a known failure from an unknown remote outcome. Supported inputs SHALL use native decoding and timing when splitting is requested.

#### Scenario: Timeout after upload

- **WHEN** a request times out after the provider may have received audio
- **THEN** the item has an unknown outcome and requires an explicit retry

#### Scenario: Existing output or duplicate basename

- **WHEN** output already exists or different selected files share a basename
- **THEN** each result has an unambiguous identity and preserves existing data

#### Scenario: Oversized input

- **WHEN** an audio file exceeds the selected endpoint's upload limit and splitting is disabled or unsupported
- **THEN** the worker reports that item and preserves results from other files
- **AND** it sends no truncated upload

#### Scenario: User enables long-file chunking

- **WHEN** the operator enables splitting for a supported oversized audio file
- **THEN** the worker sends bounded 16 kHz mono PCM WAV parts with known source offsets
- **AND** splitting requires no external media program
- **AND** the source, per-part results, and durable outcomes remain available

#### Scenario: A chunk fails after other chunks complete

- **WHEN** a part fails or has an unknown outcome
- **THEN** the worker identifies the affected part and keeps completed parts and other batch results
- **AND** retrying an unknown outcome requires explicit approval

#### Scenario: A long session exceeds memory

- **WHEN** a synthetic two-hour source is split
- **THEN** processing uses bounded memory and temporary disk space
- **AND** the acceptance test records resource use and source offsets

### Requirement: Native audio extraction for submitted video

The worker SHALL extract only the first embedded audio track of a recognized local video using native Windows media services from Rust. Both local and endpoint recognition SHALL consume only the extracted audio.

#### Scenario: User selects an MP4

- **WHEN** an operator approves a supported MP4 or explicitly submits it through a compatible scripted command
- **THEN** the selected recognizer consumes mono 16 kHz PCM16 audio
- **AND** endpoint mode uploads WAV parts
- **AND** the source file remains intact and transcription starts no playback

#### Scenario: User cancels at the guided prompt

- **WHEN** the operator declines the batch
- **THEN** recognition does not start and the worker sends no request

#### Scenario: Video has no usable audio

- **WHEN** Windows cannot open a usable audio stream or decoding fails
- **THEN** the worker reports the file error and never uploads the container
- **AND** a file with no audio stream causes zero requests
- **AND** other eligible files can proceed

#### Scenario: Windows lacks Media Foundation

- **WHEN** Windows lacks the required video components
- **THEN** the worker can still start and transcribe supported audio files
- **AND** video import reports the missing component

### Requirement: Bounded video audio jobs

The worker SHALL stream video audio using bounded buffers. It SHALL split video audio independently of the container size and the splitting option for audio files. Each WAV upload SHALL fit the configured limit.

#### Scenario: Decoded audio exceeds one request

- **WHEN** the decoded audio exceeds the endpoint limit
- **THEN** each consecutive WAV part fits the limit including its header
- **AND** offsets count decoded frames from the first selected audio sample

#### Scenario: User repeats a completed native job

- **WHEN** source content, recognition settings, conversion method, and output directory match a completed job
- **THEN** the worker reuses the result without another upload
- **AND** the result remains distinct from an earlier whole-video upload

### Requirement: Portable recorder and worker delivery

The Windows package SHALL contain the native application, compatible worker entry point, operator guide, example config, and dependency notices. Built-in transcription and media processing SHALL run without separately installed Python, a shell script, or FFmpeg. Optional model weights MAY remain separate. The repository SHALL retain the legacy C++ recorder, its build path, and its dedicated package guide.

#### Scenario: Extract and run the portable package

- **WHEN** an operator extracts the package on a supported Windows installation and opens the native batch workflow
- **THEN** the application finds its packaged components without a PATH dependency
- **AND** the guide explains model selection, local and endpoint modes, UI and agent controls, and compatible scripts
- **AND** a clean extraction contains no private configuration, recordings, or transcripts
