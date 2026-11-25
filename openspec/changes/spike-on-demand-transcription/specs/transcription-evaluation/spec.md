## ADDED Requirements

### Requirement: Explicit transcription requests

The spike SHALL evaluate transcription initiated by an explicit user command for one or more selected finalized files or confirmed sessions. It SHALL evaluate batch operation without automatic recognition or network requests during recording or browsing.

#### Scenario: Transcribe a selected recording

- **WHEN** the user invokes transcription for one finalized file with an endpoint and model
- **THEN** the job sends only the selected file to that endpoint and produces a result for that file

#### Scenario: Batch transcription

- **WHEN** the user submits several finalized files or confirmed sessions in one command
- **THEN** the job processes each selected item and reports its independent success or failure

#### Scenario: No implicit upload

- **WHEN** the user records, stops, browses, or opens a recording without invoking transcription
- **THEN** the application sends no transcription request

### Requirement: Custom endpoint compatibility

The spike SHALL test a configurable OpenAI-compatible transcription endpoint with a user-supplied model, optional authorization, and support for path prefixes. It SHALL test multipart audio submission and plain text or JSON text responses.

#### Scenario: Reverse-proxy prefix

- **WHEN** the configured base is `https://example.test/team/v1/`
- **THEN** the request uses `/team/v1/audio/transcriptions` exactly once and sends the selected model and audio

#### Scenario: Local service without credentials

- **WHEN** the user selects a loopback HTTP endpoint and supplies no key
- **THEN** the request contains no authorization header and its model and audio remain available to the selected endpoint

#### Scenario: Redirect to another host

- **WHEN** the endpoint responds with a redirect to a different host
- **THEN** the job stops and reports the redirect without forwarding audio or credentials

#### Scenario: Unsupported optional parameters

- **WHEN** a provider explicitly rejects a selected optional language, prompt, or timestamp parameter
- **THEN** the job reports the unsupported setting and any fallback is recorded with its effective parameters

### Requirement: Preserve source and timeline identity

The spike SHALL evaluate source-labeled transcripts using confirmed track grouping. It SHALL preserve simultaneous timed segments, recording offsets, and original per-source results without inventing missing timestamps or speaker identities.

#### Scenario: Application and microphone speak together

- **WHEN** timed app and microphone responses contain overlapping segments
- **THEN** the merged result retains both segments with their source labels and offsets

#### Scenario: Untimed or ambiguous input

- **WHEN** the provider returns text without times or the track-to-session mapping is ambiguous
- **THEN** the result identifies missing timing or requires an explicit mapping and does not fabricate order or speaker identity

#### Scenario: Removed silence or pause

- **WHEN** a recording omitted a paused or silent interval and no elapsed-time map exists
- **THEN** the transcript identifies its timestamps as recording time rather than wall-clock time

#### Scenario: Combined and separate tracks

- **WHEN** selected files include both a combined track and its confirmed separate tracks
- **THEN** the job requires a distinct choice before transcribing duplicate audio

### Requirement: Safe batch execution

The spike SHALL evaluate bounded concurrency, configurable endpoint limits, resumable job state, and cancellation. It SHALL preserve originals and completed output and distinguish retryable failures from requests with unknown remote completion.

#### Scenario: Request times out after upload

- **WHEN** the client cannot determine whether the provider processed the audio
- **THEN** the item is marked unknown and the client waits for an explicit retry rather than automatically resending it

#### Scenario: Large recording

- **WHEN** a selected recording exceeds a configured endpoint limit
- **THEN** the job uses validated decodable chunks with offsets or reports the failing item while preserving other results

#### Scenario: Cancel and resume

- **WHEN** the user cancels a partially completed batch and later resumes it
- **THEN** completed results remain intact, pending work can resume, and an unknown remote request is not silently repeated

### Requirement: Measured transcription quality

The spike SHALL compare a documented local baseline with available candidate models on manually corrected held-out speech. It SHALL report accuracy, false speech, timing, resource use, and cost with per-source results and failure cases.

#### Scenario: Candidate quality gate

- **WHEN** a candidate is evaluated on the held-out set
- **THEN** the report compares its word error, silence hallucinations, and technical terms against the baseline using the documented decision thresholds

#### Scenario: Optional text correction

- **WHEN** AI correction is evaluated
- **THEN** the raw transcript remains available and the report identifies changed numbers, names, additions, omissions, and any error increase

#### Scenario: Missing evaluation coverage

- **WHEN** an endpoint or representative recording is unavailable
- **THEN** the report marks the corresponding result untested and avoids claiming a quality improvement
