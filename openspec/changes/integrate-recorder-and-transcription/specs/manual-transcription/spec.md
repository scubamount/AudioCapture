## ADDED Requirements

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
