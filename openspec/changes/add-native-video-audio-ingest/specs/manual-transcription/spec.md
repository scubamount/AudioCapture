## ADDED Requirements

### Requirement: Native audio extraction for submitted video

The worker SHALL transcribe recognized local video files through the existing completed-file workflow. It SHALL use only the first embedded audio stream.
It SHALL decode this stream locally in Rust through native Windows media APIs. It SHALL send only audio data to recognition.

#### Scenario: User selects an MP4

- **WHEN** the user approves a guided batch containing a supported MP4 or submits it through the scripted command
- **THEN** the worker sends mono 16 kHz PCM16 WAV audio to the configured endpoint
- **AND** it preserves the source file and produces the usual transcript outputs without playback

#### Scenario: User cancels at the guided prompt

- **WHEN** the user declines the batch
- **THEN** the worker sends no recognition request

#### Scenario: Video has no usable audio

- **WHEN** the worker cannot open a supported audio stream or encounters a decoding error
- **THEN** it reports a file error without uploading the video container
- **AND** a file with no audio stream produces no recognition request
- **AND** other eligible files in the batch can proceed

#### Scenario: Windows lacks Media Foundation

- **WHEN** the media components for video import are unavailable
- **THEN** the standalone worker can still start and transcribe supported audio files
- **AND** submitting a video reports the missing component

### Requirement: Bounded video audio jobs

The worker SHALL stream decoded audio using bounded buffers. It SHALL split video audio into WAV parts that fit the configured upload limit.
It SHALL apply this behavior regardless of the original video's size or the optional external conversion setting for oversized audio files.

#### Scenario: Decoded audio exceeds one request

- **WHEN** the selected audio exceeds the configured upload limit
- **THEN** the worker submits consecutive parts within the limit, including each WAV header
- **AND** it offsets each result by elapsed decoded audio frames from the first selected audio sample

#### Scenario: User repeats a completed native job

- **WHEN** the source content, settings, conversion method, and output directory match a completed job
- **THEN** the worker reuses the completed result without another upload
- **AND** it distinguishes the job from a previous upload of the video container

## MODIFIED Requirements

### Requirement: Explicit transcription requests

The companion command SHALL transcribe one or more selected completed audio or video files only after an explicit user invocation.

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
- **AND** includes the selected model and the applicable audio payload, including extracted WAV parts for a video

#### Scenario: Redirect response

- **WHEN** the endpoint responds with a redirect
- **THEN** the client reports the response without forwarding the audio or key

#### Scenario: Endpoint response

- **WHEN** the endpoint returns supported plain text or JSON containing text
- **THEN** the client saves the transcript and retains the raw JSON when supplied

### Requirement: Native manual entry point

The Windows recorder SHALL offer **Transcribe files…** while idle, recording, transcribing live, or finalizing another interval. It SHALL open a native multi-file picker for completed audio files, video files, and finalized recording sessions. Guided batch requests SHALL run in the sibling worker with explicit approval. Choosing an existing file is independent of starting a live transcript.

#### Scenario: Select completed tracks

- **WHEN** a user selects finished files
- **THEN** the recorder passes paths to the sibling worker with the existing guided argument semantics
- **AND** spaces, Unicode, and option-like filenames reach the worker intact
- **AND** the UI remains responsive and any active recording continues

#### Scenario: Redirect the recorder's standard streams

- **WHEN** the recorder starts with redirected streams and opens a guided console
- **THEN** the worker displays prompts and accepts input in its own console

#### Scenario: Cancel selection or fail to launch

- **WHEN** the user cancels selection or the worker cannot start
- **THEN** the selected action sends no transcription request
- **AND** the recorder reports a launch failure without changing other activities

#### Scenario: Capture without transcription

- **WHEN** a user starts, pauses, marks or stops recording without selecting live or batch transcription
- **THEN** the recorder sends zero transcription requests and starts no speech worker

#### Scenario: Select the active recording

- **WHEN** a selected item is still open for recording
- **THEN** the queue identifies the unfinished item and excludes it from uploads until it is finalized and approved
- **AND** selecting that file never silently switches to live transcription

### Requirement: Guided settings and consent

The guided worker SHALL collect a base URL, model, output folder, and optional API-key environment variable name. It SHALL offer shared-session alignment, timestamps, optional splitting for oversized audio files, and HTML review. It SHALL show the files, destination, and effective settings before asking for affirmative upload consent.

#### Scenario: Use guided defaults

- **WHEN** the user accepts the optional defaults
- **THEN** output targets a `transcripts` folder beside the first selected file
- **AND** tracks use separate timelines with timestamps and optional audio-file chunking off
- **AND** the worker enables HTML review with a fresh output filename

#### Scenario: Consent to upload

- **WHEN** the user confirms the final submission prompt
- **THEN** the worker may upload audio from the confirmed files with the shown settings
- **AND** the summary and output exclude the API key

#### Scenario: Decline or interrupt confirmation

- **WHEN** the user enters no, leaves the final answer blank, or reaches end of input before confirming
- **THEN** the worker sends zero transcription requests

#### Scenario: Use the scripted command

- **WHEN** the user invokes the worker with the existing scripted arguments
- **THEN** the command runs without guided prompts and retains its existing options and output behavior

### Requirement: Efficient local processing with equivalent results

The worker SHALL use file metadata to reject oversized audio files early when optional audio chunking is off. It SHALL read cached JSON through a buffer and preserve the existing results and recovery behavior.

#### Scenario: Reject oversized input before content work

- **WHEN** an audio file exceeds the configured limit and optional audio chunking is off
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
