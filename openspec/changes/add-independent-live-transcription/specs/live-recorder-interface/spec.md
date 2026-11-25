## Purpose

Give Windows users independent controls and visible status for audio recording, live transcription, speaker labels and file batches.

## ADDED Requirements

### Requirement: Independent native activity controls

The recorder SHALL offer **Start recording** and **Start transcription** as separate native controls. While active, each SHALL show its corresponding Stop action. The window SHALL show **Stop both** while both activities run. Each section SHALL show its own elapsed time, current state, and output path. The **Live transcription** section SHALL also show the selected scope and destination. These controls SHALL stay visible with settings collapsed.

#### Scenario: Start one activity and add the other

- **WHEN** the user starts live transcription and later selects **Start recording**
- **THEN** the live state and its elapsed time continue while recording shows its own new start time
- **AND** the same controls support starting live transcription after recording has started

#### Scenario: Stop or pause one activity

- **WHEN** the user stops recording, pauses recording, or stops live transcription while both are active
- **THEN** each activity shows its own state and the other action remains available
- **AND** recording audio finalization and transcript finalization each report completion independently
- **AND** a remaining live activity keeps its capture status visible when recording is paused or finished

#### Scenario: Stop both activities together

- **WHEN** the user selects **Stop both** while recording and live transcription are active
- **THEN** the recorder sends the same source-time cutoff to both activities
- **AND** each activity reports its own finalization progress and result

#### Scenario: Change source scope during capture

- **WHEN** a live capture already owns the selected sources and the user starts the other activity
- **THEN** the interface identifies the active scope before the new activity uses it
- **AND** it requires a fresh acquisition before a different source scope takes effect

### Requirement: Optional live settings and transcript progress

The recorder SHALL place recognition endpoint settings, speaker labels, and transcription silence settings in the live transcription area. The ordinary recording flow SHALL remain available without configuring a recognizer. The live view SHALL show a bounded amount of the readable transcript and provide actions to open the local text file and its folder. It SHALL present unresolved time ranges, finalization progress, and an actionable error when results are incomplete.

#### Scenario: Confirm live scope

- **WHEN** the user selects **Start transcription**
- **THEN** the recorder shows the selected sources, recognizer destination, and model before the worker begins processing under the confirmed configuration
- **AND** a cancelled or failed start leaves any recording active and reports the transcript state

#### Scenario: Change live speaker display

- **WHEN** the user enables or disables **Speaker labels** while live transcription runs
- **THEN** the control changes speaker analysis for subsequent intervals and shows startup or failure state when applicable
- **AND** previously accepted text and prior speaker evidence stay available
- **AND** the display distinguishes provisional speaker labels from confirmed names

#### Scenario: Finish while the provider is slow

- **WHEN** the user selects **Stop transcription** and the worker still has accepted chunks to finish
- **THEN** the recorder shows **Saving transcript** with completed or pending progress and retains access to the readable cache
- **AND** the recorder shows the final text path when it is ready, with a visible status if any intervals remain unresolved

### Requirement: Reach completed-file batch work independently

The recorder SHALL provide **Transcribe files…** from the main window. The batch view SHALL accept multiple completed files and valid session manifests. It SHALL show per-item status and provide accessible controls for starting, cancelling, and resuming eligible work. Provider setup and confirmation SHALL apply to the selected batch scope.

#### Scenario: Queue files while capture is active

- **WHEN** the user opens batch transcription during a recording or live transcription
- **THEN** the user can select completed files and inspect or start eligible jobs
- **AND** the worker identifies files still being written and allows the other eligible files to proceed
- **AND** batch controls leave the ongoing activity controls and metering responsive

### Requirement: Reflow and keyboard access

The interface SHALL prioritize source, activity, and progress controls at the top of the window. It SHALL place advanced settings in separate collapsible groups and provide vertical scrolling when needed. Visible controls SHALL have accessible names, current values, logical keyboard focus order, and visible focus indicators. State announcements SHALL report meaningful changes without announcing each transcript update or meter sample.

#### Scenario: Open the main window with both activities idle

- **WHEN** the user opens an idle recorder at a wide window size
- **THEN** the source picker and three entry points appear with settings collapsed, as in this layout sketch

```text
Sources              [Select programs…] [Recent programs ▾] [Microphone ▾]
                     Discord · Microphone

Recording            Ready · Speech (Opus 32 kbps mono)
                     [Start recording] [Recording settings ▾]

Live transcription   Ready
                     [Start transcription] [Transcription settings ▾]

Completed files      [Transcribe files…]
```

#### Scenario: Record at narrow width and different display scales

- **WHEN** the recorder is 390 or 660 logical units wide at 96, 120, 144, or 192 DPI
- **THEN** neither activity nor its Stop action overlaps adjacent controls
- **AND** the complete Opus bitrate in kbps or WAV sample rate in Hz is visible with its label and reachable while its codec is selected
- **AND** keyboard focus scrolls active controls into view when needed

#### Scenario: Use the recorder with the keyboard and a screen reader

- **WHEN** the user tabs through sources and actions, opens the batch view, and returns to the main window
- **THEN** focus follows the visual order and returns to the control that opened the view
- **AND** accessible names and status identify recording, live transcription, and batch states in addition to their visual indicators
- **AND** the accessible navigation can reach each active Stop control and its output path

### Requirement: Portable transcription settings

The recorder SHALL load and save a versioned JSON configuration for the transcription settings panel. It SHALL ship an example separate from the active configuration.

#### Scenario: Open a fresh extraction with a personal configuration

- **WHEN** the user starts the recorder with `transcription.json` beside its executable
- **THEN** it fills the endpoint, model, API key variable name, output folder, timestamps and available speaker and speech settings
- **AND** it resolves relative paths from the configuration file's directory
- **AND** each transcription interval still requires the user's approval

#### Scenario: Import or save settings

- **WHEN** the user selects **Load config…** with no live transcription active or pending
- **THEN** the recorder validates the file before applying its values together
- **AND** a canceled or invalid import preserves the current settings
- **WHEN** the user selects **Save config…**
- **THEN** the file stores the API key variable name and excludes its value

#### Scenario: Check the download

- **WHEN** the user checks a config from the command line
- **THEN** the recorder validates it and returns an exit status without opening a window or contacting the endpoint

#### Scenario: Submit completed files using the current settings

- **WHEN** the user selects **Transcribe files…** with a configured transcription panel
- **THEN** the guided worker uses the current endpoint, model, API key variable name, output folder and timestamp setting without asking for them again
- **AND** it shows the effective settings and selected files before asking permission to submit
- **WHEN** the user starts the guided worker directly
- **THEN** it loads `transcription.json` beside its executable if present, or prompts for settings if absent

#### Scenario: Inspect settings during simultaneous recording and transcription

- **WHEN** both activities are running and the user opens either settings panel
- **THEN** its settings come into view, including on a narrow window with locked controls
- **AND** the user can close the panel without stopping either activity or changing the active settings
