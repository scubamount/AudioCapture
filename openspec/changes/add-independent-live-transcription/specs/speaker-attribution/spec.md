## MODIFIED Requirements

### Requirement: Explicit offline or capability-aware processing

The companion worker SHALL support explicit single-session and batch attribution of finalized inputs. It SHALL support a configured local diarization adapter without network access. Optional live speaker analysis SHALL run in the worker only during authorized intervals. Local speaker processing SHALL send no audio or conversation context to a service. The recorder SHALL load no speaker model or perform automatic memory export.

#### Scenario: Process local results

- **WHEN** the user selects completed recognition artifacts and an installed adapter with networking unavailable
- **THEN** the worker processes each selected source locally and reports each outcome
- **AND** it neither downloads a model nor invokes an external service

#### Scenario: Select a custom endpoint

- **WHEN** the user requests speaker output from a configured compatible endpoint
- **THEN** the worker sends optional speaker fields only for a known endpoint and model capability under existing audio submission consent
- **AND** the worker validates speaker and timing fields and records their provenance

#### Scenario: Receive text only

- **WHEN** an endpoint returns text without usable speaker or timing fields
- **THEN** the worker preserves the text and can use a selected local adapter on available authorized audio
- **AND** unsupported speakers or times remain unknown without another upload

#### Scenario: Record without speaker analysis

- **WHEN** a user browses programs or records without enabling live labels or submitting an explicit offline job
- **THEN** no speaker model loads and no speaker analysis, conversation-context transfer or memory export starts
