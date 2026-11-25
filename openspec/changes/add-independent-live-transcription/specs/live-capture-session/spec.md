## Purpose

Let one capture session supply independently controlled recording and live transcription intervals.

## ADDED Requirements

### Requirement: Independent recording and transcription controls

The controller SHALL track recording and live transcription as independent consumers of the selected sources. Each individual control SHALL change only its target. When both activities are active, the controller SHALL also offer **Stop both** and use one source-frame cutoff for both intervals.

#### Scenario: Exercise every control transition

- **WHEN** the user invokes an action in the table
- **THEN** the corresponding consumers accept new input after the action succeeds

| Before | Action | After |
| --- | --- | --- |
| Idle | Start recording | Recording |
| Idle | Start transcription | Transcription |
| Recording | Start transcription | Both |
| Transcription | Start recording | Both |
| Both | Stop recording | Transcription |
| Both | Stop transcription | Recording |
| Both | Stop both | Idle |
| Recording | Stop recording | Idle |
| Transcription | Stop transcription | Idle |

#### Scenario: Stop both during a pending block

- **WHEN** the user selects **Stop both** at source frame N while both activities are active
- **THEN** both intervals end at N exclusively, even when their start frames differ
- **AND** both consumers can finalize already accepted input independently
- **AND** the controller reports each output's final status separately

#### Scenario: Start one operation alone

- **WHEN** the user starts recording alone
- **THEN** the recorder saves the selected audio and starts zero speech workers, models, or transcription requests
- **WHEN** the user starts transcription alone
- **THEN** live text can reach its final file without creating a permanent audio recording
- **AND** any temporary recognition audio follows the worker's bounded staging and cleanup rules

#### Scenario: Reject or repeat an action

- **WHEN** the second consumer fails to start or the user declines its required confirmation
- **THEN** the first consumer continues from its existing position and the interface identifies the failed action
- **WHEN** duplicate start or stop events arrive for the same consumer and interval
- **THEN** the controller creates at most one interval, writer, and finalization operation for that action

### Requirement: Shared acquisition with independent intervals

The controller SHALL acquire each selected source once while consumers share it. A capture session SHALL use one monotonic origin. Each recording interval and transcription interval SHALL have a unique identifier and a half-open range on that session's clock. Cutoffs SHALL apply at capture time, including inside a pending block.

#### Scenario: Join a consumer during speech

- **WHEN** transcription starts partway through a recording
- **THEN** the worker receives only samples captured from the admitted start frame after confirmation
- **AND** the existing recording continues without reopening its source, changing its clock, or duplicating a frame
- **WHEN** recording starts partway through transcription
- **THEN** saved audio starts at the recording boundary and the transcription interval retains its original start

#### Scenario: Stop and restart one consumer

- **WHEN** either consumer stops while the other remains active
- **THEN** it detaches at its cutoff and finalizes the accepted interval independently
- **AND** pending transcription results can finish after intake stops
- **WHEN** the user starts the stopped consumer again
- **THEN** it receives a new interval ID and output name on the continuing capture clock
- **AND** late completions for the old interval can update only that interval

#### Scenario: Release the last input

- **WHEN** the last consumer of a source stops
- **THEN** the controller promptly releases that input without waiting for a model, remote response, or text export
- **AND** an explicitly active monitor retains only its own required inputs
- **WHEN** a later start acquires new inputs after all consumers stopped
- **THEN** a new capture session gets a distinct ID and time origin

### Requirement: Output finalization and failure isolation

Each output SHALL finalize independently. Recording and transcript artifacts that overlap SHALL share stable session and interval references, even when their stops occur in a different order. Only the requested consumer SHALL pause or stop.

#### Scenario: Save matching output from staggered intervals

- **GIVEN** transcription T starts at second 0 and recording R starts at second 5
- **WHEN** recording stops at second 12 and transcription stops at second 20
- **THEN** R closes its audio and records its overlap with T as session seconds [5, 12)
- **AND** T continues through second 20 and publishes its own final text with the reference to R
- **AND** R can finish before the transcript path is final, using T's stable ID
- **WHEN** T stops first in another session
- **THEN** its text can finalize while recording continues
- **AND** the eventual recording manifest identifies each overlapping transcription interval and its bounds

#### Scenario: Pause the recording

- **WHEN** recording pauses and resumes while transcription remains active
- **THEN** recording omits the paused frames across its saved tracks
- **AND** the live worker still receives those frames within its authorized interval
- **AND** their transcript times identify positions on the session clock without claiming positions in saved audio

#### Scenario: One destination fails or stalls

- **WHEN** a writer encounters a fatal output error while transcription is active
- **THEN** the recorder identifies the affected file and ends the recording interval with a failure status
- **AND** healthy sources continue supplying the transcription interval
- **WHEN** a speech worker fails, its queue fills, or its finalizer stalls
- **THEN** capture never waits on that worker and the recorder continues with its established output and clock
- **AND** the affected transcript identifies incomplete coverage

#### Scenario: A selected source disappears

- **WHEN** a selected source exits while another selected source supplies audio
- **THEN** both consumers keep the surviving source on its existing clock and identify the loss
- **AND** neither consumer substitutes system audio or a new process based only on a matching PID
- **WHEN** every selected source is lost
- **THEN** the controller detaches both inputs, reports the cause, and finalizes each accepted interval with its coverage status
