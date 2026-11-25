## Purpose

Schedule approved finished-file transcription alongside live work while preserving individual results and recovery state.

## ADDED Requirements

### Requirement: Explicit batch scope

The interface SHALL let the user queue multiple completed files or finalized recording sessions. A batch SHALL carry its confirmed input list, destination, model, options, and output location. Selecting remembered programs or opening the queue SHALL create no upload.

#### Scenario: Queue completed work during capture

- **WHEN** the user selects completed files or sessions while a separate recording or live transcription is active
- **THEN** the queue previews the actual input files and their destination before submission
- **AND** an open recording or unfinished live cache remains ineligible
- **AND** accepting or cancelling the batch leaves active recording and live transcription states unchanged

#### Scenario: A session contains a mix and separate tracks

- **WHEN** a selected session offers both forms
- **THEN** the queue shows which inputs will be transcribed and detects duplicate coverage before submission
- **AND** it includes intentional duplicate coverage only after explicit selection
- **AND** unrelated sessions retain separate timeline origins

#### Scenario: Request batch speaker attribution

- **WHEN** the user enables local speaker attribution for selected items
- **THEN** the worker applies it only to those items with the required artifacts and available adapter
- **AND** a missing adapter or failed attribution preserves each successful recognition result and reports the affected item

### Requirement: Bounded and resumable scheduling

Batch execution SHALL reuse the existing job identities, per-item outcomes, file-size limits, locks, completed-result cache, and explicit unknown-outcome retry policy. One on-demand worker SHALL schedule live and batch work submitted through the UI or its guided consoles. Independently launched scripted CLI processes retain their existing scheduling. The queue SHALL persist state transitions without a resident worker when idle.

#### Scenario: Request a live interval while a batch owns the only slot

- **WHEN** a confirmed live start finds the only request slot occupied
- **THEN** the UI shows **Waiting for batch request** and accepts cancellation of the pending live start
- **AND** no live audio is admitted or shown as covered before the slot becomes available
- **AND** the in-flight batch request finishes or reaches its absolute deadline, with a default maximum of 120 seconds from dispatch
- **AND** the scheduler starts live intake at the actual admission frame and reports an unknown batch outcome when applicable

#### Scenario: Reserve capacity during an active live interval

- **WHEN** live and batch work share a destination
- **THEN** the scheduler reserves one slot for live work, even between spoken phrases
- **AND** a single-slot queue visibly defers remaining batch requests until live intake and pending recognition have ended
- **AND** a higher configured concurrency limit permits eligible batch requests in spare slots
- **AND** all destinations share a finite worker-wide request limit and audio staging budget
- **AND** a request deadline, batch failure, or pending start never stalls recording

#### Scenario: Submit the same work twice

- **WHEN** two queue entries select the same completed content and request settings within the same cache scope
- **THEN** the existing job lock prevents duplicate concurrent uploads
- **AND** the second entry reuses the first successful result
- **AND** changing the model, content, or relevant request options cannot reuse an incompatible result

#### Scenario: Pause, cancel, resume, or restart

- **WHEN** the user pauses a batch
- **THEN** current requests may finish, and pending items remain queued until explicit resume
- **WHEN** the user cancels a batch or reopens it after an application restart
- **THEN** no further audio is sent until the user explicitly selects eligible work to resume
- **AND** each item shows pending, active, completed, cancelled, failed, or unknown status as applicable
- **AND** recovery preserves completed items and requires a separate retry decision for a possible remote completion

#### Scenario: An input changes or one item fails

- **WHEN** a queued input changes after confirmation, disappears, or exceeds the per-file limit with chunking disabled
- **THEN** the worker reports the affected item without sending changed or truncated content under the earlier approval
- **AND** other eligible items can finish
- **AND** replacing the input requires new confirmation
