## Purpose

Recognize speech and attribute speakers on demand with local Rust models. Preserve explicit provider choices, source timing, and recovery.

## ADDED Requirements

### Requirement: Built-in local speech recognition

The Windows application SHALL provide a local speech recognition backend implemented in Rust. The same backend SHALL support completed audio, supported video audio, and live capture. The bundled Rust worker MAY run without a window to isolate inference from capture and the UI. Built-in local workflows SHALL require no external executable, Python installation, or local HTTP bridge. A compatible HTTP transcription endpoint SHALL remain an explicitly selected alternative.

#### Scenario: Recognize sound from an MP4 offline

- **WHEN** a user or agent with an installed model chooses the local backend and submits a supported MP4
- **THEN** the native video importer extracts its first embedded audio stream and passes bounded PCM blocks to the recognizer
- **AND** it writes the usual text and structured transcript with timing on the decoded source clock
- **AND** no media is played, no network socket is opened, and no audio upload file is created

#### Scenario: Recognize live Discord and microphone audio locally

- **WHEN** the user starts a live interval for Discord and a microphone with a supported local model
- **THEN** the recognizer consumes the approved streams and publishes recoverable results with their source IDs and frame ranges
- **AND** starting and stopping transcription stays independent of starting and stopping recording
- **AND** the recognition queue reports gaps when it cannot keep up, while capture continues without waiting

#### Scenario: Select an endpoint explicitly

- **WHEN** a user or agent selects a compatible HTTP provider for a job
- **THEN** the application shows the effective destination and uses the existing consent, credential, and transport rules
- **AND** local backend errors never trigger a transfer to that provider

#### Scenario: Process a long local recording

- **WHEN** a user submits a supported file to a local model that cannot read the entire recording in one inference window
- **THEN** the worker streams consecutive windows under the local memory budget and joins results on the source clock
- **AND** an endpoint profile's upload size limit does not reject the local job

### Requirement: Explicit model lifecycle and compatibility

The UI and agent interface SHALL list installed models and their capabilities. Packs SHALL identify format, languages, features, provenance, license, and content digest. The runtime SHALL load models only on demand under a bounded inference budget. It MAY reuse a pack across jobs with the same model identity.

#### Scenario: Install or import a compatible model

- **WHEN** a user approves a model install or imports a compatible local pack
- **THEN** the application verifies its files before it makes the pack available for jobs
- **AND** an interrupted or invalid install leaves any previously verified installation usable
- **AND** opening the app or listing models does not start a download

#### Scenario: First use without an existing profile

- **WHEN** the operator opens transcription on a clean installation without a compatible model
- **THEN** the app selects local mode and offers model installation or offline import
- **AND** selecting a remote endpoint requires an explicit provider choice

#### Scenario: Load a former local Whisper profile

- **WHEN** the user loads a profile for the existing faster-whisper bridge
- **THEN** the application preserves its endpoint, model, and output settings
- **AND** it offers a way to select a compatible native model explicitly and keeps the previous endpoint profile available
- **AND** when the user selects incompatible CTranslate2 weights for native inference, it explains the format mismatch and leaves that native model unavailable

#### Scenario: Idle or recording-only use

- **WHEN** the user starts the recorder, records audio alone, or opens a saved transcript
- **THEN** the application performs no model load or inference for that action
- **AND** an idle worker can release its model and inference buffers within the published resource policy

#### Scenario: Select a language unsupported by the pack

- **WHEN** a user requests a language that the model does not support
- **THEN** setup identifies the limitation before work starts and offers a compatible installed model if one is available
- **AND** the job waits for a supported model and language selection

### Requirement: Optional speech detection and speaker attribution

Native speech detection and speaker models SHALL be optional. Speech detection SHALL preserve the source clock and record skipped ranges. Attribution SHALL use known source assignments when available. For mixed audio, it SHALL emit transcript-scoped speaker labels and retain overlaps and uncertainty. Contextual names SHALL follow the existing evidence and review rules.

#### Scenario: Quiet speech follows silence

- **WHEN** speech detection is active and a reference recording contains silence followed by quiet speech
- **THEN** recognized speech and any skipped ranges retain decoded or capture source positions
- **AND** missing recognition coverage remains distinct from a range identified as silence
- **AND** changing detection settings does not edit the original recording

#### Scenario: Label two known source tracks

- **WHEN** the job receives separate Discord and microphone tracks
- **THEN** it retains the source labels and reports overlapping timed turns independently
- **AND** source labels alone do not assign a personal identity

#### Scenario: Label turns on one mixed track

- **WHEN** the user requests mixed-track attribution with an installed speaker pack
- **THEN** the worker uses bounded local analysis and keeps stable transcript-scoped label IDs across analysis windows when the evidence supports a match
- **AND** ambiguous turns remain unassigned or provisional
- **AND** names inferred from context cite their supporting turns and preserve explicit reviewer decisions

### Requirement: Consistent native outcomes and resource limits

The worker SHALL commit local results through the same job and transcript state progression used by the UI and agent actions. It SHALL identify cached results by input, provider, effective options, conversion method, and exact model revision. It SHALL keep inference, decoder waits, model loading, and disk or network activity off the capture and UI threads. It SHALL report the processing and resource limits for each qualified model profile.

#### Scenario: Stop a local job

- **WHEN** the user or agent cancels a queued or active job
- **THEN** the application stops scheduling further inference windows and reports cancellation at a bounded checkpoint
- **AND** previously committed text stays readable with incomplete coverage identified
- **AND** dependent exports do not treat incomplete work as a completed result

#### Scenario: Lose access to a model or decoder

- **WHEN** a model becomes unavailable, an input codec is unsupported, or the inference worker fails
- **THEN** the affected job reports the cause and the recovery action
- **AND** other eligible jobs, recording, and completed results remain available

#### Scenario: Qualify a native model on Windows

- **WHEN** a model profile is assessed for release
- **THEN** maintainers test a fresh Windows package with an imported pack and network egress disabled
- **AND** they verify timestamps, quiet speech, separate-source speech, and bounded cancellation against labeled fixtures
- **AND** they record recognition error, processing speed, cold start, warm memory, and capture frame accounting against the reference baseline
- **AND** live or speaker features remain marked for evaluation until their published quality and performance gates pass
