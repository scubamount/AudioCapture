## ADDED Requirements

### Requirement: Document recording profile behavior

The recorder documentation SHALL explain the six built-in profiles, the first-run choice, manual encoding, and named saved setups.

#### Scenario: Choose an encoding for a conversation

- **WHEN** a reader consults the guide to choose an encoding
- **THEN** the guide lists each profile's format, channel count, and bitrate or sample rate
- **AND** it explains DTX timing, the effect of **Skip silence**, and the limits of the speech comparison

#### Scenario: Reuse an encoding or a setup

- **WHEN** a reader wants to retain sources and routes or restore a full configuration
- **THEN** the guide explains the effect of a recording profile and the effect of a named saved setup

### Requirement: Record the evidence and its scope

The project SHALL document the controlled offline comparison and the reason for the first-run profile. It SHALL keep private audio and exact transcript differences in private output.

#### Scenario: Review the measured basis for the initial profile

- **WHEN** a reader follows the profile evidence in the source-linked LLM wiki
- **THEN** the reader can identify the input scope, candidate settings, local ASR configuration, storage comparison, and timing check
- **AND** the wiki links to the relevant implementation, specifications, and tests
- **AND** the summary describes observed differences and separates the measured result from broader quality claims

#### Scenario: Inspect a public example or package

- **WHEN** a reader opens the repository docs, examples, or distributable files
- **THEN** audio assets and screenshots use privacy-safe source material
- **AND** private recordings, exact transcript differences, and private paths are absent
