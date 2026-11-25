## Purpose

Give Windows users native controls for recording, transcription, review, recovery, and memory operations.

## ADDED Requirements

### Requirement: Operate built-in workflows in the Windows application

The Rust application SHALL provide native controls for every supported operator action and its parameters. It SHALL present inputs, validation, progress, results, and recovery in the application. Completing a built-in workflow SHALL require neither a console nor editing a command, manifest, or JSON document.

#### Scenario: First use on a fresh extraction

- **WHEN** an operator opens the recorder and configures transcription
- **THEN** they can choose local inference or a compatible endpoint, select a model, and choose an output folder
- **AND** they can load, edit, validate, and save a portable config from labeled controls
- **AND** missing components or invalid fields have an actionable status in the application

#### Scenario: Configure capture and output

- **WHEN** an operator configures a recording
- **THEN** they can select programs, recent-program rules, system audio, microphone, monitoring device, source gains, and mixed or separate outputs
- **AND** they can select the format, channels, WAV sample rate, bitrate, FLAC compression, speech DTX profile, and silence removal where applicable
- **AND** they can apply a built-in profile or save, load, and delete a named preset

#### Scenario: Control an active session

- **WHEN** the operator starts or stops recording or live transcription in either order
- **THEN** the UI shows the state of each activity, source levels, current output, and available text
- **AND** pause, resume, markers, live settings, cancellation, and opening results remain available in their supported states
- **AND** inspecting a setting does not interrupt recording or submit audio

#### Scenario: Set recognition options

- **WHEN** the operator configures live recognition or a completed-file batch
- **THEN** the form exposes applicable model, endpoint prefix, credential variable name, language, timestamps, provider profile, and upload limit
- **AND** it exposes session alignment, duplicate-source policy, audio splitting, optional speaker analysis, and review output where applicable
- **AND** it distinguishes unavailable capabilities from enabled settings and keeps credential values private

### Requirement: Native queue and recovery controls

The application SHALL expose submitted batches, per-file progress, errors, and output paths in native controls. It SHALL use the same persistent job identities and recovery rules as the agent interface. Decoding, inference, transfers, and long disk operations SHALL run outside the window message thread and the audio capture thread.

#### Scenario: Manage work while recording

- **WHEN** a batch runs during capture
- **THEN** the operator can inspect, pause, resume, or cancel it from the queue
- **AND** the UI stays responsive and live recognition retains its admission priority
- **AND** submitting or resuming work a second time respects existing job locks and completed results

#### Scenario: Reopen after interruption

- **WHEN** the operator reopens the application after an interruption
- **THEN** they can inspect batch state, failed items, and work with an unknown remote outcome
- **AND** retrying an unknown outcome requires explicit authorization
- **AND** they can list interrupted live intervals, publish available text, or confirm that they want to discard an interval

### Requirement: Native transcript review and speaker tools

The application SHALL provide forms to prepare, analyze, refine, and review speaker sessions. It SHALL preserve the source text, source timeline, revision history, evidence, and uncertainty when a user edits the reviewed result.

#### Scenario: Prepare or refine a speaker session

- **WHEN** an operator selects completed sources or a finalized live interval
- **THEN** they can pair audio with recognition artifacts, assign roles, select canonical sources, and choose a shared or separate timeline
- **AND** they can select naming context, enable inferred names, and choose supported recognition or diarization options
- **AND** they can refine retained audio using its matching timeline and inspect per-source outcomes

#### Scenario: Review and correct transcript text

- **WHEN** the operator opens a completed transcript
- **THEN** they can search, inspect sources and times, replay a timed selection on request, correct text, and export reviewed data
- **AND** untimed text and overlapping turns remain visible
- **AND** the application preserves raw recognition output and supports optional HTML review export

#### Scenario: Edit speaker decisions

- **WHEN** the operator reviews speakers and the evidence for an inferred name
- **THEN** they can confirm, reject, or restore a name, rename or unlink a person, and reassign a turn
- **AND** they can split or merge turns and speaker groups with labeled controls
- **AND** the application validates the expected revision before applying the edit and explains a conflict

### Requirement: Native memory export and import

The application SHALL provide native forms for the supported memory export and reference import operations. The same scope selection, approval, revision, and replay rules SHALL apply to UI and agent actions.

#### Scenario: Export an approved scope

- **WHEN** the operator selects sessions, a destination, and an incremental cursor or snapshot
- **THEN** they can inspect the proposed export and approve or cancel it
- **AND** the application shows the manifest and cursor after a successful export

#### Scenario: Import an exported batch

- **WHEN** the operator selects an export manifest and an import destination
- **THEN** the application displays the applied changes or the existing receipt for a replay

### Requirement: Quiet execution and consistent controls

The application SHALL use Rust entry points or packaged native components for built-in operations. UI actions SHALL start any background workers without a visible console or shell command. UI, headless, and MCP callers SHALL use the same validation, operation semantics, and durable state for the operations they share.

#### Scenario: Operate without a terminal

- **WHEN** the recorder starts without console handles and the operator uses any built-in workflow
- **THEN** the workflow collects all input and displays outcomes in native controls
- **AND** an operator can complete built-in work without starting a Python service, installing FFmpeg, or opening a command prompt

#### Scenario: An agent and operator inspect the same job

- **WHEN** one interface starts an explicitly authorized job and another inspects its ID
- **THEN** both report the same source selection, effective settings, state, and output location
- **AND** either interface can apply the supported controls to that job under the same recovery rules
