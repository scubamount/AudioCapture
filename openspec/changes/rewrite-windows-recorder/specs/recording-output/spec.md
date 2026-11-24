## ADDED Requirements

### Requirement: File modes and a shared timeline

The recorder SHALL offer separate files, a combined file, and both. It SHALL align each source on a common session timeline.

#### Scenario: Two sources and three modes

- **WHEN** an app and microphone record in separate, combined, and both modes in three sessions
- **THEN** the output contains two isolated files, one mixed file, and three files respectively

#### Scenario: A source starts late

- **WHEN** a selected source produces its first packet after another source
- **THEN** its audio begins at the corresponding offset in the mix without shifting earlier speech

#### Scenario: Pause and resume

- **WHEN** the user pauses all sources, waits, resumes, and stops
- **THEN** the outputs omit the paused interval and retain alignment across the transition

#### Scenario: Skip silence

- **WHEN** silence skipping is enabled during a mixed and separate recording
- **THEN** the same silent intervals are removed from aligned outputs and speech from either source is preserved

### Requirement: Usable local recordings

The recorder SHALL write playable WAV, MP3, Opus, and FLAC files. It SHALL expose the selected codec settings and provide PCM WAV for transcription.

#### Scenario: Codec options

- **WHEN** the user selects MP3, Opus, or FLAC
- **THEN** the recorder offers the upstream bitrate choices for lossy codecs or compression levels 0 through 8 for FLAC

#### Scenario: Transcription handoff

- **WHEN** the app plays one spoken phrase and the microphone supplies a different phrase in combined WAV mode
- **THEN** a local transcription tool can import the finalized file and recover both phrases

#### Scenario: Rapid recording restart

- **WHEN** the user stops and starts another recording within the same second in the same directory
- **THEN** each session receives distinct filenames and both outputs remain available

#### Scenario: Write failure

- **WHEN** the destination becomes unwritable during recording
- **THEN** the recorder reports the affected path and preserves previously finalized files
