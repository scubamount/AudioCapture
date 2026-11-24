## ADDED Requirements

### Requirement: Reusable and compatible presets

The recorder SHALL save current preferences and named presets. It SHALL import supported legacy values without modifying the original settings or presets.

#### Scenario: Restore a Discord preset

- **WHEN** the user imports a legacy preset with Opus, separate files, and a selected output directory
- **THEN** the recorder restores those values and presents matching current process candidates for capture

#### Scenario: Saved identity unavailable

- **WHEN** a stored microphone or selected program is missing or ambiguous
- **THEN** the recorder identifies the unresolved selection and requires an explicit replacement before capturing that source

#### Scenario: Manage named presets

- **WHEN** the user saves two presets, loads one, and deletes the other
- **THEN** the loaded settings match the saved values and only the chosen preset is removed
