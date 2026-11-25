## Purpose

Provide optional provisional speaker labels during a call and refine those labels using available authorized audio.

## ADDED Requirements

### Requirement: Optional local speaker processing

The companion worker SHALL start local speaker analysis only for sources explicitly selected for an active live transcript. The recorder SHALL load zero speaker models. Speaker processing SHALL default to off.

#### Scenario: Enable speakers during a call

- **WHEN** the user enables local speaker labels during live transcription
- **THEN** the worker starts analysis at the enable boundary, using the selected local models
- **AND** existing text remains available while the models load
- **AND** the action creates no audio recording, model download, remote speaker request, or memory export

#### Scenario: Turn labels off and on

- **WHEN** the user disables labels while transcription continues
- **THEN** the worker stops accepting speaker audio at that boundary and releases its speaker buffers and model process
- **AND** subsequent text retains source information and carries no local speaker assignment
- **AND** re-enabling labels starts a new analysis interval without analyzing the intervening audio

#### Scenario: Stop recording first

- **WHEN** recording stops while live transcription and speaker labels remain active
- **THEN** the worker continues labeling new authorized audio

#### Scenario: Stop transcription first

- **WHEN** live transcription stops
- **THEN** speaker analysis admits no more audio, completes or times out outstanding work, and leaves any active recording running

### Requirement: Bounded local inference

The worker SHALL reuse one initialized model instance per active configuration across bounded analysis windows. It SHALL cap window length, lookback, queued audio, speaker state, inference concurrency, and adapter output. It SHALL enforce a processing deadline outside the capture thread.

#### Scenario: Select a file-only adapter

- **WHEN** the selected adapter supports only the released one-source file protocol
- **THEN** the worker offers offline refinement for saved audio and reports live labels unavailable
- **AND** it continues transcription without launching that adapter for each new window

#### Scenario: Process multiple windows

- **WHEN** consecutive windows from selected sources reach the local adapter
- **THEN** the worker provides bounded samples, source IDs, and original sample offsets
- **AND** consecutive windows reuse loaded model weights and keep each source's clustering state separate
- **AND** overlapping context does not duplicate text or advance the source clock twice

#### Scenario: Speaker processing falls behind

- **WHEN** the queue fills, a model is unavailable, or the adapter fails its deadline
- **THEN** the worker reports the affected source and interval as lacking speaker coverage
- **AND** recognition, text-cache updates, and recording can continue independently
- **AND** the worker retains no extra audio beyond its configured bounds to catch up

### Requirement: Evidence for provisional speaker labels

The worker SHALL attach provisional, source-scoped anonymous IDs to timed turns. It SHALL preserve raw transcript text and retain concurrent speaker activity. Each speaker assignment SHALL reference its transcript span, source interval, and analysis revision.

#### Scenario: Local cluster numbers change at a window boundary

- **WHEN** adjacent windows assign different cluster numbers to speech in their common interval
- **THEN** the worker uses shared speech to preserve an ID when it supports one unambiguous match
- **AND** competing candidates remain unresolved, or receive distinct provisional IDs when evidence distinguishes them
- **AND** a later correction revises the assignment on the existing turn

#### Scenario: A speaker returns after a gap

- **WHEN** a speaker resumes after silence without a shared speech anchor
- **THEN** reuse of an ID requires a unique match against qualified, bounded acoustic evidence from that source and analysis interval
- **AND** an unsupported match remains uncertain
- **AND** acoustic matching state expires when the interval ends and cannot link speakers across sessions

#### Scenario: Overlap or timing prevents assignment

- **WHEN** several speakers overlap, or the recognizer supplies text without usable word or segment timing
- **THEN** the worker keeps each recognized span once and exposes the uncertainty
- **AND** neither a microphone label, a repeated cluster number, nor voice similarity alone establishes a person's identity

### Requirement: Optional refinement of saved audio

The worker SHALL finalize cached text independently of an optional speaker refinement pass. Refinement SHALL require an explicit selection and locally available audio with a verified time map for the requested intervals.

#### Scenario: Refine available intervals

- **WHEN** a user selects a finalized transcript and matching retained audio
- **THEN** the offline worker can reconcile local labels against the saved audio and publish a new machine-generated revision with its provenance
- **AND** the worker preserves original text, prior revisions, and explicit reviewer decisions
- **AND** existing contextual naming and memory export policies apply independently

#### Scenario: Transcript outlasts the recording

- **WHEN** retained audio covers only part of the transcript or the user saved text alone
- **THEN** the worker identifies which intervals support refinement and keeps the other text and available live labels
- **AND** finalization requires neither re-recording nor an upload

### Requirement: Qualification of live speaker results

A live backend SHALL pass the reference-host, accuracy and coverage gates before release as a qualified option. The evaluation SHALL compare live windows with the released whole-source backend on identical audio, reference timings, and scoring rules.

#### Scenario: Evaluate attribution and resource use

- **WHEN** maintainers evaluate a model configuration on fixed regression calls and held-out calls
- **THEN** they report diarization error, isolated-turn recall, overlap error, label changes across windows, and unresolved coverage separately for mixed and separate tracks
- **AND** they report cold start, p95 label delay, inference time per audio second, peak process-tree memory, queue occupancy, and lost capture frames on the reference host
- **AND** sustained processing time divided by two-source call duration is at most 0.8 on the designated host
- **AND** p95 provisional label delay is at most 15 seconds from the end of eligible speech after warm-up
- **AND** the report counts unresolved and late turns even when no label arrives within the target
- **AND** the report identifies the hardware, model hashes, window settings, and limits of each corpus
- **AND** claims about natural calls require held-out human recordings with reference speaker turns
