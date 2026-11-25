## Purpose

Control silence removal and speech gating independently, and locate transcript words on the original or saved timeline.

## ADDED Requirements

### Requirement: Separate controls for recording size and recognition input

The application SHALL treat **Skip silence** in a recording, Opus DTX, and speech gating for transcription as separate settings. Speech gating SHALL default to off. Existing recording profiles and saved silence settings SHALL keep their meaning.

#### Scenario: Choose how to reduce recording size

- **WHEN** the user enables recording silence removal
- **THEN** all saved tracks and the mix omit the same quiet blocks on the recording timeline
- **AND** a block with audible speech from either selected source stays on every saved track
- **WHEN** the user selects Opus DTX with recording silence removal off and leaves recording unpaused
- **THEN** the recording preserves its duration, including quiet intervals
- **AND** neither choice enables transcription, changes its gate, or removes recognized words from a transcript

#### Scenario: Measure the storage tradeoff

- **WHEN** a benchmark applies each silence control separately to the same source samples
- **THEN** it reports saved audio bytes, playable duration, recognition input duration, and final UTF-8 text bytes separately
- **AND** it compares quiet speech and exact transcript changes against the ungated run

#### Scenario: Gate recognition without changing saved audio

- **WHEN** transcription speech gating is on and recording silence removal is off
- **THEN** the worker can omit confidently quiet input from recognition while the recording retains those samples
- **AND** it records the omitted time ranges as gate decisions
- **WHEN** transcription speech gating is off and recording silence removal is on
- **THEN** live recognition receives the authorized source audio before recording silence removal
- **AND** toggling the transcription gate does not change the recording's PCM sample sequence

#### Scenario: Protect weak speech and word boundaries

- **WHEN** evaluation audio contains a quiet speaker, a leading consonant, a short pause, and a trailing syllable
- **THEN** the worker uses bounded pre-roll and post-roll around detected speech
- **AND** uncertain audio goes to recognition
- **AND** pre-roll and post-roll stay inside the selected source and authorized transcription interval
- **AND** overlapping gate windows emit each retained input region once before recognition chunking
- **WHEN** a fully quiet interval exceeds both margins
- **THEN** the worker omits its confidently quiet interior and reports the source duration and duration submitted

### Requirement: Traceable clocks for recording and transcription

The application SHALL identify positions by capture session, source, interval, and integer frame ranges. The common source clock SHALL use 48,000 frames per second. Ranges SHALL include the first frame and exclude the ending frame. A recording map SHALL relate kept source frames to logical playback time. Recognition input SHALL carry its own map from chunk offsets to source time.

#### Scenario: Seek through a cut

- **GIVEN** a recording starts at session second 10 and shared silence removal discards session seconds [13, 18)
- **WHEN** a word starts at session second 19
- **THEN** its playback position in each saved track is second 4
- **AND** a word at session second 16 has no playback position in that recording
- **AND** a saved marker after the cut uses the same playback clock
- **AND** published transcript times retain their session positions across the cut

#### Scenario: Play a saved file at another sample rate

- **WHEN** an accepted word maps to second 4 in a 16 kHz WAV file
- **THEN** its playback position is frame 64,000 even though capture uses 48 kHz
- **AND** playback links for compressed files account for known codec delay or identify seek uncertainty

#### Scenario: Map source turns across chunks and rates

- **WHEN** the worker resamples a source, processes successive chunks with overlap, or omits a gated interval
- **THEN** each chunk carries its source ID, input rate, session-frame spans, and the source range it owns
- **AND** the worker maps recognized offsets through the retained spans using integer sample counts
- **AND** neighboring chunks reconcile duplicated boundary words before publishing committed text
- **AND** missing or uncertain timing stays marked as such
- **AND** a turn spanning an omitted region retains its distinct mapped spans or an explicit timing uncertainty

#### Scenario: Preserve simultaneous speakers

- **WHEN** microphone and program speech overlap for one second
- **THEN** each source retains its own attributed text on the same session second
- **AND** combined elapsed duration counts their overlapping coverage once
- **AND** per-source processing totals count the frames actually processed for each source

#### Scenario: Distinguish missing signal from a deliberate cut

- **WHEN** a source drops packets, its timestamp becomes unreliable, or the recognition queue drops input
- **THEN** metadata identifies the source, cause, and measured range when its bounds are known
- **AND** unresolved bounds remain uncertain
- **AND** the text and recording maps distinguish loss from a pause, a silence decision, or time outside a consumer interval
- **AND** unaffected source and file positions remain stable

#### Scenario: Bound the cost of mapping

- **WHEN** a long run contains continuous audio and occasional changes of retention
- **THEN** the recorder streams adjacent spans with the same mapping as runs, preserving each mapping change
- **AND** retained in-memory history stays bounded as duration grows
- **AND** recording alone performs zero recognition gating, inference, or transcript chunk mapping
