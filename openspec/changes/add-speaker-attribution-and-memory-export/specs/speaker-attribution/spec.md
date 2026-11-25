## ADDED Requirements

### Requirement: Explicit offline or capability-aware processing

The companion worker SHALL support explicit single-session and batch attribution of selected finalized inputs. It SHALL support a configured local diarization adapter without network access. The recorder SHALL perform no attribution, model loading, context transfer, or memory export during capture or browsing.

#### Scenario: Process local results

- **WHEN** the user selects completed recognition artifacts and a locally installed adapter with networking unavailable
- **THEN** the worker processes each selected source using its local artifacts and reports each outcome independently
- **AND** the worker neither downloads a model nor invokes an external service

#### Scenario: Select a custom endpoint

- **WHEN** the user requests speaker output from an explicitly configured OpenAI-compatible transcription endpoint
- **THEN** the worker sends optional diarization fields only for a known endpoint/model capability profile under existing audio submission consent
- **AND** the worker validates returned speaker and timing fields and records their provenance

#### Scenario: Receive text only

- **WHEN** a custom endpoint returns usable text without usable speaker or timing fields
- **THEN** the worker keeps all text and may use a selected local adapter on the original source
- **AND** unsupported turn assignments or times remain unknown without another upload

### Requirement: Independent source and speaker attribution

The worker SHALL assign stable, session-local anonymous speaker IDs independently of track labels and named identities. It SHALL use one canonical representation per confirmed recording interval and flag ambiguous mappings for review.

#### Scenario: Several participants speak through Discord

- **WHEN** two remote speakers alternate on one Discord track and a third speaker talks on a microphone track
- **THEN** supported turns distinguish the two remote speakers while retaining the same source ID
- **AND** microphone activity retains its own source and requires separate evidence to acquire a person identity

#### Scenario: Select mixed and separate files

- **WHEN** a confirmed session contains its mix and a complete set of separate tracks
- **THEN** the default selects the separate tracks and records the excluded mix as an alternate
- **AND** an explicit mix selection processes only the mix

#### Scenario: Source coverage is uncertain

- **WHEN** partial stems, overlapping system and application captures, or uncertain session membership prevent unique coverage
- **THEN** the worker requests a canonical source choice before combining those intervals
- **AND** the worker can keep unrelated sessions separate

### Requirement: Honest timing and concurrent speech

The worker SHALL preserve source-relative speech intervals, including overlaps, on the saved recording timeline. It SHALL retain unattributed activity and every available word when evidence cannot identify a speaker or time.

#### Scenario: Concurrent activity with uncertain words

- **WHEN** two speaker intervals overlap and a coarse ASR segment cannot assign words uniquely
- **THEN** the result retains both activity intervals and stores the text once with unresolved attribution
- **AND** absent word-level timing never creates invented word boundaries

#### Scenario: Merge chunk offsets

- **WHEN** valid intervals from multiple chunks have known source frame offsets
- **THEN** each interval maps to the source timeline using its offset exactly once
- **AND** repeated provider cluster numbers in separate chunks do not establish speaker continuity

#### Scenario: Missing alignment

- **WHEN** timestamps are invalid or a verified shared clock or wall-clock map is unavailable
- **THEN** the worker preserves valid source-relative information and marks missing timing or alignment explicitly
- **AND** the review avoids assigning a chronological position to untimed text from another track

### Requirement: Evidence-based contextual naming

The worker SHALL infer speaker names from explicitly selected conversation text and identity records. It SHALL preserve all supporting and conflicting evidence turns, text spans, source revisions, confidence tiers, and decision status. An optional policy SHALL display high-confidence direct introductions or corroborated conversational matches automatically as inferred names. Only a reviewer action SHALL confirm an identity or create a durable cross-session person link.

#### Scenario: A participant directly introduces herself

- **WHEN** a uniquely attributed speaker says “My name is Nia” without conflicting evidence and automatic display is selected
- **THEN** review displays Nia as inferred on this speaker's turns without a confirmation prompt
- **AND** review offers the cited introduction while retaining the anonymous speaker ID

#### Scenario: Use relevant prior conversation

- **WHEN** an approved prior transcript contains an unambiguous self-introduction and a verified participant mapping links its speaker to the current speaker
- **THEN** automatic display may use the prior name without a separate name confirmation
- **AND** review identifies the earlier evidence turn, revision, text span, and correspondence decision
- **AND** the resulting name remains inferred until confirmed

#### Scenario: Reject misleading names

- **WHEN** a speaker quotes “I am Nia,” mentions an absent Nia, addresses Nia, or has conflicting direct introductions
- **THEN** none of those cases automatically names the speaker
- **AND** an alias, shared display name, source label, or similar voice alone cannot create a durable person link

#### Scenario: Infer a name from ordinary conversation

- **WHEN** two distinct speakers address Nia by name in separate exchanges and only the same anonymous speaker answers each request directly
- **AND** an approved independent source corroborates the participant or role and no candidate or cue conflicts
- **THEN** automatic display may label that speaker Nia as inferred without a separate confirmation
- **AND** review cites both requests, both replies, and the independent corroboration

#### Scenario: Competing ordinary cues

- **WHEN** replies overlap, several speakers answer, names conflict, or only one addressed request and a roster name are available
- **THEN** those cues cannot assign an automatic name through the conversational match rule
- **AND** review retains unresolved candidates and their permitted evidence

### Requirement: Reversible local identity review

The review SHALL show anonymous IDs, candidates, confidence, and cited evidence. It SHALL support confirmation, rejection, correction, splitting, merging, unlinking, and reassignment through revisioned decisions. Naming and diarization SHALL operate without a voiceprint registry, voice synthesis, or a language model.

#### Scenario: Undo a mistaken cross-session match

- **WHEN** a reviewer removes a person link and assigns the correct person
- **THEN** review shows the updated identity and retains the original turn and speaker IDs
- **AND** the affected identity references receive a new revision for downstream correction

#### Scenario: Invalidate stale name evidence

- **WHEN** a cited self-introduction is corrected, withdrawn, or excluded from the permitted context
- **THEN** the worker removes the dependent inferred label unless other permitted evidence independently supports it
- **AND** the prior decision remains traceable in local revision history
