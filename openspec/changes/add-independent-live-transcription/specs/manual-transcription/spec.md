## MODIFIED Requirements

### Requirement: Native manual entry point

The Windows recorder SHALL offer **Transcribe files…** while idle, recording, transcribing live, or finalizing another interval. It SHALL open a native multi-file picker. Guided batch requests SHALL run in the sibling worker with explicit approval. Choosing an existing file is independent of starting a live transcript.

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
