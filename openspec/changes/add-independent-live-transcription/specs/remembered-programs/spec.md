## Purpose

Let users select a familiar program quickly and identify it again when its process changes.

## ADDED Requirements

### Requirement: Quick source selection

The recorder SHALL offer a program picker, a recent-program action, and an optional microphone selector in the same source section. The picker SHALL support the active-audio filter and distinguish programs with the same name by executable location and current process details, when available. It SHALL preserve manual multiple-program selection and explicit system audio selection.

#### Scenario: Pick a program and microphone

- **WHEN** the user selects a running Discord process and a microphone in the source section
- **THEN** the source summary shows Discord and the selected microphone before either activity starts
- **AND** either start action uses the confirmed scope without opening recording or transcription settings for the other activity

#### Scenario: Use a previously used program

- **WHEN** the user invokes a recent-program action and exactly one eligible running program matches its saved identity
- **THEN** the recorder selects that program and shows it in the source summary
- **AND** the action can restore the previously used microphone by endpoint ID when it is available
- **AND** the user must select a start action to acquire or send audio

### Requirement: Opt-in automatic selection and local history

The recorder SHALL store a bounded local list of recently used programs after a recording or live transcription successfully starts with those sources. It SHALL store identities and display names for selection. It SHALL provide **Forget** for one entry and **Clear recent programs** for the list. Automatic selection SHALL require a separate preference that defaults to off. Named setups SHALL keep their own lifecycle.

#### Scenario: Reopen with automatic selection enabled

- **WHEN** automatic selection is enabled and an idle recorder has no pending manual or named-setup selection
- **THEN** the recorder preselects the last used program if the last source choice included a single program and exactly one eligible process tree matches
- **AND** a source discovery update can complete the same pending preselection while the recorder remains idle
- **AND** if that program is absent or the last choice used several programs, the recorder shows available recent choices for explicit selection
- **AND** the user sees the selected source and can change it before starting either activity

#### Scenario: Clear or forget a source

- **WHEN** the user forgets a recent program or clears the list
- **THEN** the removed entries disappear and cease to qualify for automatic selection
- **AND** the recorder clears any idle preselection based solely on a removed entry
- **AND** an active source continues until its activity stops or the source becomes unavailable

### Requirement: Verify the remembered program identity

The recorder SHALL verify an application identity before automatically selecting a process. For a desktop executable, the match SHALL use an OS-reported canonical image path and a verified executable fingerprint. For a packaged application, it MAY use an OS-reported package family and application identifier. The recorder SHALL validate the selected PID and process creation time again at capture activation.

#### Scenario: Restart a verified program

- **WHEN** the remembered application restarts with a different PID and one process tree still matches its saved identity
- **THEN** the idle recorder can select the new instance under the automatic selection preference
- **AND** a process restart during capture leaves the affected source unavailable until the user starts a new acquisition with a verified choice

#### Scenario: Resolve ambiguity or a changed executable

- **WHEN** two eligible process trees match a remembered identity, the executable has changed, or the recorder cannot verify the identity
- **THEN** the recorder shows a source choice that needs review and waits for an explicit selection
- **AND** executable basename, window title, most recent activity, and a reused PID never qualify a process by themselves
- **AND** other eligible sources can start after the user explicitly removes the unresolved choice

#### Scenario: Restore a legacy setup or a missing microphone

- **WHEN** a stored setup supplies a basename without a verified image identity, or its microphone endpoint is unavailable
- **THEN** the recorder shows candidates or the missing endpoint for review
- **AND** the user can confirm an eligible program or select another microphone before including it in a capture
- **AND** source selection never widens to system audio or selects a default microphone as a fallback

### Requirement: Bound discovery work

The recorder SHALL reuse one discovery snapshot for the picker, activity filter, and remembered source matching. It SHALL use change notifications or a rate-limited refresh and coalesce redundant refresh requests. Identity checks SHALL run outside the UI thread for relevant candidates. Discovery SHALL release its resources when application activity no longer needs it.

#### Scenario: Start many programs and open the picker

- **WHEN** the process list changes rapidly while the user opens the picker or selects a program
- **THEN** the interface remains responsive and shows the latest available snapshot
- **AND** the recorder bounds pending discovery work and checks source identity again if the selection is stale
- **AND** periodic refreshes reuse verified fingerprints for unchanged executable instances
