# recorder-resilience Specification

## Purpose

Preserve usable recordings and accurate timelines through source loss, encoder errors, and capture discontinuities.

## Requirements

### Requirement: Recoverable Opus checkpoints

The recorder SHALL complete an Ogg page and flush buffered Opus data within each second of active accepted audio. It SHALL report checkpoint errors.

#### Scenario: Recorder process exits during capture

- **GIVEN** a writable local destination and successful checkpoints
- **WHEN** the recorder process exits abruptly after at least four seconds of continuous synthetic input
- **THEN** an Ogg Opus decoder can decode an intact prefix through the last checkpoint
- **AND** the prefix lags accepted input by no more than one second plus two 20 ms frames and the reported encoder lookahead

#### Scenario: Stop after a checkpoint

- **WHEN** a user stops normally after a checkpoint at a non-frame boundary
- **THEN** the file has a valid final granule and decodes to the requested sample count

#### Scenario: Checkpoint write fails

- **WHEN** a checkpoint page or flush fails
- **THEN** the recorder identifies the output and reports the error

### Requirement: Tolerate late source packets

The recorder SHALL preserve the shared timeline when a packet arrives after committed audio. It SHALL report per-source dropped audio without stopping healthy sources.

#### Scenario: Packet straddles the committed cursor

- **WHEN** a source delivers a packet that begins before the cursor and ends after it
- **THEN** the recorder drops only the committed prefix and inserts the remaining samples at their original time
- **AND** it reports the source and lost time interval while continuing other sources

#### Scenario: Packet is entirely late

- **WHEN** a packet ends before the committed cursor
- **THEN** the recorder drops it, records the loss, and keeps other sources running
- **AND** the missing source interval remains silent without shifting later samples

#### Scenario: Invalid packet

- **WHEN** a packet contains incomplete frames or an impossible future timestamp
- **THEN** the recorder reports the invalid packet and does not treat it as ordinary late delivery

### Requirement: Recover from capture discontinuities

The recorder SHALL report source discontinuities and timestamp uncertainty while preserving available audio from other sources.

#### Scenario: A timestamped packet follows a discontinuity

- **WHEN** one source reports a discontinuity with a usable timestamp and another source continues
- **THEN** the recorder retains the usable packet at its reported timeline position
- **AND** reports the source and preserves the healthy source

#### Scenario: A discontinuity has no reliable timestamp

- **WHEN** a source reports an invalid timestamp during a discontinuity
- **THEN** the recorder reports uncertain timing and does not claim a measured missing interval
- **AND** continues unaffected sources

### Requirement: Continue recording after a source exits

The recorder SHALL preserve healthy source tracks when another capture source exits during a session. It SHALL report source loss and stop when no source remains.

#### Scenario: One of two sources exits

- **WHEN** a selected source exits while another source remains active
- **THEN** the recorder names the lost source and keeps recording the healthy source
- **AND** preserves the shared timeline and identifies uncertainty about the lost interval

#### Scenario: The last source exits

- **WHEN** the last available source exits
- **THEN** the recorder stops and reports the loss

#### Scenario: An output writer fails

- **WHEN** an output writer returns a fatal error
- **THEN** the recorder reports the affected output and stops the session
- **AND** does not retry or duplicate the failed block

### Requirement: Prevent overlapping mixed input and monitor feedback

The recorder SHALL identify known overlapping selected sources before mixing. It SHALL prevent a system-loopback monitor route to the same endpoint.

#### Scenario: Overlapping sources feed a mix

- **WHEN** the user selects system audio and an app known to play on that endpoint for mixed or monitored output
- **THEN** the recorder identifies the overlap and requests a non-overlapping selection before starting

#### Scenario: Parent and child process trees overlap

- **WHEN** the user selects both a process and its descendant for a mix
- **THEN** the recorder identifies the overlap before starting
- **AND** separate-only recording remains available

#### Scenario: Monitor endpoint matches the pinned capture endpoint

- **WHEN** the actual system capture endpoint and actual monitor endpoint match at start or after a routing change
- **THEN** the recorder prevents or disables that monitor route and reports the affected endpoint
- **AND** an independent file recording continues after a change during capture

#### Scenario: Endpoint identity is indeterminate

- **WHEN** the actual captured or monitoring endpoint identity cannot be established
- **THEN** the recorder reports the uncertainty and prevents or disables monitoring until the route is known to be safe
- **AND** an independent file recording continues

### Requirement: Per-source recording activity

The interface SHALL show each selected source's peak level and recent activity at a bounded update rate. It SHALL expose accessible source names and levels.

#### Scenario: One active and one silent source

- **WHEN** the application source has signal and the microphone source is silent
- **THEN** the two indicators show their respective activity without marking the silent source as failed

#### Scenario: Source stops

- **WHEN** an active source exits or stops sending audio
- **THEN** its indicator becomes quiet and the interface identifies any reported source failure

#### Scenario: Source gain and clipping

- **WHEN** a source supplies a quiet signal before gain and its amplified signal clips
- **THEN** the interface exposes pre-gain activity and a retained clipping indication for that source

#### Scenario: Mono downmix cancellation

- **WHEN** a mono recording receives substantial opposing-polarity stereo input
- **THEN** its source indicator reports cancellation without changing the captured signal

### Requirement: Recording timeline markers

The recorder SHALL save explicitly requested markers in a session-specific Audacity-compatible label sidecar on the recording output timeline.

#### Scenario: Mark a session

- **WHEN** the user adds a marker during a file recording
- **THEN** its sidecar contains its sanitized label and the corresponding recorded-output position
- **AND** no transcription request occurs

#### Scenario: Mark near a removed interval

- **WHEN** a marker occurs while recording is paused or silence is skipped
- **THEN** its output position maps to the adjacent retained boundary
- **AND** later markers use the same shortened timeline as the audio

#### Scenario: Sidecar collision or write failure

- **WHEN** the marker destination exists or becomes unwritable
- **THEN** the recorder preserves the existing file and reports the failure

#### Scenario: Marker processing and audio rendering

- **WHEN** markers are queued while a source is recording
- **THEN** audio rendering, encoding, and sidecar flushing do not hold the marker queue lock
- **AND** a bounded queue reports an overflow or persistence failure

### Requirement: Optional compact speech Opus

The recorder SHALL offer an explicitly selected mono Opus speech profile using VOIP mode and DTX at a supported selectable Opus bitrate. Existing Opus mode and defaults SHALL remain available.

#### Scenario: Select the speech profile

- **WHEN** a user selects `Opus speech (mono, DTX)` and an available bitrate
- **THEN** the recorder creates mono `.opus` output using VOIP and DTX
- **AND** retains shared-timeline and interruption-recovery behavior

#### Scenario: Keep ordinary Opus settings

- **WHEN** the user selects regular Opus or loads existing settings
- **THEN** the recorder uses the existing Opus mode and preserves its settings

### Requirement: Preserve downsampled WAV boundaries

The WAV writer SHALL compensate its resampler delay and include its filter tail while preserving intended output duration.

#### Scenario: Boundary impulses

- **WHEN** a synthetic input has impulses near the first and final frames and is written at 16 kHz or 44.1 kHz
- **THEN** decoded output retains both boundary signals within the target-rate frame resolution
- **AND** its duration and alignment match the intended shared timeline across chunk boundaries

#### Scenario: WAV rollover

- **WHEN** a downsampled WAV crosses a part boundary
- **THEN** its parts preserve the total frame count and continuous filter state

### Requirement: Preserve imported gains

The recorder SHALL retain legacy gain values at 1% resolution or better until the user changes them.

#### Scenario: Imported gain is unchanged

- **WHEN** a 34% legacy gain is loaded, recording starts, and settings are saved or the window closes without editing gain
- **THEN** the gain remains 34% when the settings are reloaded

### Requirement: Partial discovery failure

The recorder SHALL list process snapshots independently of active audio-session enumeration and report incomplete audio activity information.

#### Scenario: One endpoint fails

- **WHEN** an endpoint or session fails during active-audio enumeration
- **THEN** the unfiltered process list remains available
- **AND** the interface reports that activity information is incomplete

### Requirement: Resource improvements preserve output

The recorder SHALL retain only necessary encoded packet bytes and process pause transitions without repeatedly copying the complete history. It SHALL preserve audio content, timeline, and supported recording modes.

#### Scenario: Compare packet allocation

- **WHEN** the same ten-second 48 kHz synthetic fixture is encoded as 32 kbps mono using the original and improved allocations with the same checkpoint policy
- **THEN** the improved allocation has a lower measured peak of live Rust heap bytes
- **AND** the benchmark reports peak live Rust heap bytes and file size for both builds
- **AND** decoded audio, sample count, and timing remain equivalent

#### Scenario: Multiple pauses

- **WHEN** two sources record through repeated pauses with silence skipping enabled
- **THEN** the same timeline intervals are removed from each track and the mixed output

### Requirement: Package build identity

The packaging helper SHALL bind the executable and dependency notices to the same locked source build.

#### Scenario: Source changes after a build

- **WHEN** source or the lockfile changes after the executable was built
- **THEN** packaging rebuilds with the portable release configuration or refuses the stale executable
