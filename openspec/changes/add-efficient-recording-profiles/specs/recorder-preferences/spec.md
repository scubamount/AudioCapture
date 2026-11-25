## ADDED Requirements

### Requirement: Built-in recording profiles

The Windows recorder SHALL offer an immutable list of encoding profiles with distinguishable labels. It SHALL keep the manual encoding controls available.

#### Scenario: View available profiles

- **WHEN** the idle user opens the recording profile selector
- **THEN** it lists regular 24 and 32 kbps mono Opus, VOIP with DTX at 24 and 32 kbps mono, regular 96 kbps stereo Opus, and 16 kHz mono WAV
- **AND** the profile entries cannot be renamed or deleted through saved setup controls

#### Scenario: Apply a built-in profile

- **WHEN** the idle user selects a built-in recording profile
- **THEN** the recorder applies its format, channel count, and bitrate or sample rate
- **AND** it retains inactive codec settings, selected inputs, output mode, folder, gains, monitor settings, and pause and silence settings

#### Scenario: Decode a DTX profile with silence skipping off

- **WHEN** the user records with either DTX profile and silence skipping is off
- **THEN** the recorder writes a standard Ogg Opus file with the established recording timeline
- **AND** DTX does not remove elapsed time from that timeline

### Requirement: Derive the profile from encoding settings

The recorder SHALL derive the profile display from the current format, channel count, and active codec setting after a manual edit or setup load.

#### Scenario: Match a built-in after a manual edit

- **WHEN** an idle user manually sets regular Opus to 32 kbps mono
- **THEN** the recorder shows the matching recording profile
- **AND** inactive codec settings and other capture settings do not affect the match

#### Scenario: Display a custom encoding

- **WHEN** the current encoding settings do not match a built-in profile
- **THEN** the recorder shows **Custom** and retains the current settings

### Requirement: First-run profile and settings compatibility

The recorder SHALL apply regular 32 kbps mono Opus when current and legacy active settings are both absent. It SHALL retain existing settings and named preset behavior.

#### Scenario: Open the recorder with clean settings

- **WHEN** a user starts the recorder without current or legacy active settings
- **THEN** the active encoding is regular 32 kbps mono Opus
- **AND** the user can change the profile or edit the encoding before recording

#### Scenario: Open a version 1 file or import legacy settings

- **WHEN** the recorder reads current version 1 or supported legacy settings with missing fields
- **THEN** missing fields use their historical defaults
- **AND** present values and the existing source resolution behavior are preserved

#### Scenario: Load a named saved setup

- **WHEN** the user loads a current or supported legacy named setup
- **THEN** the recorder restores the full settings that the setup contains
- **AND** the displayed recording profile reflects those encoding settings
- **AND** deleting or replacing the setup leaves the built-in profile list available
