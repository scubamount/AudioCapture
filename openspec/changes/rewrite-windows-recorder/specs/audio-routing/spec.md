## ADDED Requirements

### Requirement: Explicit capture scope

The recorder SHALL enumerate application PIDs with names and available window titles. It SHALL support an active-audio filter, explicit system capture, and optional microphone selection.

#### Scenario: Selected process isolation

- **WHEN** one selected app and an unrelated app play different tones while the selected microphone supplies a third signal
- **THEN** decoded tracks contain only their assigned sources, including the selected app's process tree

#### Scenario: Start a second session

- **WHEN** the user stops one selected-app session and starts another for a different app
- **THEN** only the new selection contributes to the second session

#### Scenario: Source unavailable

- **WHEN** activation fails or a selected process exits
- **THEN** the recorder identifies the affected source and never substitutes whole-system audio

#### Scenario: Explicit system capture

- **WHEN** the user explicitly selects whole-system capture
- **THEN** the recorder labels the source as system audio and captures output from the selected endpoint

### Requirement: Monitoring and gains

The recorder SHALL support independent app and microphone gains, a selected monitoring output, and a monitor-only mode.

#### Scenario: Monitor without saving

- **WHEN** the user enables monitor-only and selects an output device
- **THEN** the device plays the selected audio and the session creates no recording files

#### Scenario: Monitor unavailable

- **WHEN** the monitor device disappears during a file recording
- **THEN** the recorder reports the monitoring failure and continues writing available source audio
