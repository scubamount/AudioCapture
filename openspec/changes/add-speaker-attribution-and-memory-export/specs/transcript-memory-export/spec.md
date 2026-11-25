## ADDED Requirements

### Requirement: Complete source-backed transcript

The worker SHALL produce a versioned machine-readable local transcript for each selected session. It SHALL preserve every canonical source text, including unknown, overlapping, untimed, and unaligned material. It SHALL identify missing sources, chunks, and recognition failures explicitly.

#### Scenario: Reconstruct a complete conversation

- **WHEN** a session contains overlapping timed turns, an unassigned speaker, and an untimed passage
- **THEN** reconstructing by source spans reproduces each canonical raw source text exactly once, including its separators
- **AND** each record references its session, source, timing basis, and underlying recognition artifact
- **AND** raw and reviewed text remain distinguishable

#### Scenario: Retain recognition gaps

- **WHEN** a selected source fails recognition or a chunk is unavailable
- **THEN** the transcript reports partial coverage and identifies the failed source or known missing interval
- **AND** successful, silent, missing, and untimed inputs remain distinguishable

### Requirement: Reviewed local export

The worker SHALL export only an explicitly approved selection to a local destination. It SHALL preview full or partial coverage, redactions, and identity policy. The default SHALL export approved text and anonymous speakers, adding person identity only for confirmed links. Raw audio, voice features, secrets, local paths, and unapproved context SHALL remain outside the export.

#### Scenario: Preview and cancel

- **WHEN** a user previews export and declines or cancels
- **THEN** the worker publishes no batch and leaves the previous cursor unchanged

#### Scenario: Export an inferred label deliberately

- **WHEN** the user enables provisional names for an approved scope containing a high-confidence inferred name
- **THEN** the export may include that name without per-speaker confirmation, with inferred status and permitted evidence references
- **AND** no canonical person identity is asserted for this inference
- **AND** the default policy excludes inferred-name metadata while preserving approved spoken text

#### Scenario: Restrict a private passage

- **WHEN** an exported turn cites evidence outside the approved scope
- **THEN** the export omits external private text, identifying metadata, and unapproved evidence references
- **AND** review can redact the approved turn independently of its canonical local transcript

### Requirement: Stable and versioned incremental updates

The worker SHALL provide scoped snapshot and delta exports with stable session, source, speaker, and turn IDs. It SHALL publish ordered, versioned upserts and retractions atomically. Importers SHALL apply each batch idempotently and invalidate dependent memories affected by changes.

#### Scenario: Replay, resume, and reject a gap

- **WHEN** a consumer imports a snapshot, a valid delta, and an exact replay of that delta
- **THEN** the consumer holds one current record per entity and its checkpoint advances once for that delta
- **AND** a missing predecessor, corrupt payload, unsupported schema major, or same-revision conflict leaves state and checkpoint unchanged

#### Scenario: Correct text or identity

- **WHEN** a previously exported turn receives a text correction or an approved person link changes
- **THEN** the next delta supplies the affected replacements under stable IDs with newer revisions
- **AND** the importer invalidates prior derived material referencing superseded revisions

#### Scenario: Split a turn or withdraw the whole session

- **WHEN** a reviewer replaces a turn with two turns
- **THEN** one atomic batch retracts the old ID and upserts two new IDs with predecessor references
- **AND** subsequent session withdrawal emits retractions for all previously visible entities in that stream
- **AND** a consumer that applies withdrawal removes the affected records and invalidates dependent derived material

#### Scenario: Recover after interruption

- **WHEN** export or import stops before its transaction commits
- **THEN** replay from the prior checkpoint produces the same records and cursor as uninterrupted processing
- **AND** a cursor outside retained history requires a fresh snapshot with reconciliation of previously held records
