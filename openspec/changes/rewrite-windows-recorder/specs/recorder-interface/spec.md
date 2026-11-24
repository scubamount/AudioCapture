## ADDED Requirements

### Requirement: Legible recording controls

The interface SHALL give each visible control enough space for its label, current value, focus indicator, and activation target.

#### Scenario: Reported Opus recording view

- **WHEN** the user selects Opus, the active-audio filter, and separate files at a window size near 1024 × 768 pixels
- **THEN** the filter, Opus bitrate, process gain, output path, and Stop control occupy separate reachable bounds

#### Scenario: Resize or change display scaling

- **WHEN** the user resizes the window or switches between 100%, 125%, 150%, and 200% display scaling
- **THEN** the interface reflows or scrolls before controls overlap, including with microphone and monitoring options expanded

### Requirement: Meaningful units and keyboard operation

The interface SHALL label bitrate in kbps and sample rate in Hz. Each visible setting SHALL expose an accessible name, role, and value.

#### Scenario: Switch codecs with the keyboard

- **WHEN** the user tabs to Format and switches from Opus to FLAC
- **THEN** the bitrate control leaves the focus order and the FLAC compression level becomes reachable

#### Scenario: Record at a chosen PCM rate

- **WHEN** the user selects WAV and 16000 Hz
- **THEN** the selected value remains legible and the decoded output reports 16000 samples per second

### Requirement: Transport and progress

The interface SHALL show recording state, source names, output paths, and progress. The user SHALL retain access to pause, resume, and stop.

#### Scenario: Restore during capture

- **WHEN** the user minimizes the recorder to the tray and restores it during capture
- **THEN** the same session continues and its current transport state and file progress appear
