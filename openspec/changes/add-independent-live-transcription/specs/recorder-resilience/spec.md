## MODIFIED Requirements

### Requirement: Continue recording after a source exits

The recorder SHALL preserve healthy source tracks after another capture source exits. It SHALL report source loss. The session SHALL stop accepting new frames when the last available source exits. A recording output failure SHALL stop the recording consumer independently of a healthy transcription consumer.

#### Scenario: One of two sources exits

- **WHEN** a source exits while another remains active
- **THEN** the recorder identifies the lost source, retains the shared clock, and continues capturing the healthy source
- **AND** active outputs identify the affected range and any uncertainty

#### Scenario: The last source exits

- **WHEN** the last available source exits
- **THEN** active consumers seal their input and report the loss
- **AND** the recorder closes available audio and the worker finalizes available text with its coverage status

#### Scenario: An output writer fails

- **WHEN** a recording writer returns a fatal error
- **THEN** the recorder reports the affected output and stops the recording interval without retrying the failed block
- **AND** a separately authorized live transcription continues on healthy sources
- **AND** acquisition ends when its last consumer detaches
