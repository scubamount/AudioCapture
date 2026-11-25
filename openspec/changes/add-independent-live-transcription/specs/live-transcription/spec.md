## Purpose

Save a recoverable text transcript while selected sources are active and publish it when the user stops transcription.

## ADDED Requirements

### Requirement: Consent for one live interval

Live recognition SHALL start only after the user confirms the selected sources, destination, model, text output folder, and effective audio retention setting. Confirmation SHALL cover one live interval. Only the companion worker SHALL contact the transcription endpoint.

#### Scenario: Start live text on selected sources

- **WHEN** the user confirms Discord and a microphone for live transcription
- **THEN** the worker accepts frames only after confirmation and admission to the live request slot
- **AND** the actual admission frame defines the start boundary
- **AND** the recorder shows the live state and the selected destination
- **AND** sources added later require fresh confirmation before their audio can enter a request

#### Scenario: Decline a transfer or change its destination

- **WHEN** the user cancels confirmation, restores a saved setup, starts recording alone, or opens a recovered transcript
- **THEN** that action sends zero transcription requests
- **WHEN** the user changes the recognition endpoint or model during an interval
- **THEN** the worker finishes or cancels the old scope before sending audio under a newly confirmed scope
- **AND** a subsequent Start after Stop requires confirmation for its new interval

#### Scenario: Protect credentials

- **WHEN** a compatible endpoint requires a token
- **THEN** the worker uses the configured credential reference and the existing transport policy
- **AND** arguments, text files, journals, status messages, and logs exclude the token value

### Requirement: Bounded recognition with compatible endpoints

The worker SHALL support near-live recognition through bounded multipart requests to the configured base URL's `audio/transcriptions` route. It SHALL accept compatible local or remote endpoints. It SHALL preserve the existing path-prefix, TLS, loopback HTTP, response-size, and redirect checks.

#### Scenario: A service supports file uploads

- **WHEN** a service accepts multipart audio and returns plain text or JSON with text
- **THEN** the worker submits short, independently decodable chunks during capture and emits results as they arrive
- **AND** operation requires neither a streaming socket nor partial-token support
- **AND** each chunk respects the selected duration limit
- **AND** the audio payload, including its container, fits the configured upload byte limit

#### Scenario: The endpoint stalls while recording continues

- **WHEN** processing exhausts the configured ingress capacity or an in-flight request reaches its deadline
- **THEN** capture continues on its own clock without waiting for recognition, storage, or the network
- **AND** the worker records each unprocessed source-frame range as a gap and shows the lag or failure
- **AND** retained audio, in-flight requests, and temporary audio files remain within the published resource budgets

#### Scenario: Capture speech without saving a recording

- **WHEN** only live transcription is active
- **THEN** the worker uses bounded volatile audio and, if necessary, bounded private temporary chunks within the confirmed retention setting
- **AND** it removes temporary audio after use or during recovery of its own abandoned session
- **AND** it creates no full-session audio recording or unbounded retry spool

### Requirement: Stable results across boundaries

The worker SHALL give each chunk a stable identity from its session, live interval, source, frame range, input content, and request settings. It SHALL retain source-clock coverage, timing quality, and update order independently of recognition text. Each committed segment SHALL have one durable identity.

#### Scenario: A reply arrives late or twice

- **WHEN** results arrive out of capture order or the worker replays a completed response
- **THEN** each committed segment appears once in capture order within its source
- **AND** overlapping speech on distinct sources remains distinct
- **AND** the worker records a gap if an earlier result stays unavailable past the bounded ordering window
- **AND** an accepted later correction references the original segment or gap identity

#### Scenario: Speech crosses a request boundary

- **WHEN** adjacent requests share analysis context
- **THEN** each source frame has one owning output interval
- **AND** valid timing maps recognized speech back to that interval
- **AND** a provider with untimed text uses nonoverlapping recognition payloads and preserves repeated phrases
- **AND** the worker labels chunk coverage as such and leaves unavailable word times unknown

#### Scenario: A speaker label or partial hypothesis changes

- **WHEN** a backend revises a provisional result
- **THEN** the live view replaces that provisional result under the same identity
- **AND** only committed words enter the readable text cache
- **AND** a subsequent speaker correction preserves committed words and the original source identity

### Requirement: Recoverable text during capture

The worker SHALL automatically maintain a readable UTF-8 `.partial.txt` and a versioned recovery journal for each live interval. It SHALL append new committed text incrementally. The journal SHALL record durable segment identities, source-frame coverage, request outcomes, and a text checkpoint. The worker SHALL enforce finite flush and checkpoint intervals.

#### Scenario: Read the transcript mid-call

- **WHEN** the worker commits new words
- **THEN** a reader can open the partial file during capture and see the new lines within the specified flush interval
- **AND** later commits append without rewriting earlier text or retaining the entire transcript in memory
- **AND** provisional words and uncertain speaker names remain distinguishable in the live view

#### Scenario: Recover after a crash between two writes

- **WHEN** the worker restarts after a journal write, a partial text write, or a final rename
- **THEN** recovery validates the last durable checkpoint and repairs or replays its bounded tail
- **AND** opening recovery sends zero requests and leaves audio acquisition stopped unless another consumer already owns it
- **AND** every durably committed segment appears once and torn UTF-8 records never appear as completed speech
- **AND** repeated recovery produces the same text and identities

#### Scenario: The endpoint may have received a request

- **GIVEN** the worker durably records the request identity and uncertain outcome before transmission
- **WHEN** a request ends with an unknown remote outcome
- **THEN** recovery reuses a valid cached response when available
- **AND** resubmission requires an explicit retry and availability of the approved audio range
- **AND** unavailable audio leaves a visible gap with its source, range, and reason

### Requirement: Cache ownership and cleanup

The worker SHALL keep its recovery cache private to the current Windows user. Startup cleanup SHALL operate only on verified application-owned files inside that cache. The worker SHALL preserve recoverable text after failed publication.

#### Scenario: Publish or discard a recovery

- **WHEN** final publication succeeds and the worker verifies the published interval and content
- **THEN** it releases redundant text, cached responses, and journal data used only for recovery
- **WHEN** the user explicitly discards an abandoned recovery
- **THEN** the worker removes only cache files owned by that interval
- **AND** both cleanup paths preserve finalized audio, published text, and unrelated files

#### Scenario: Clean abandoned temporary audio

- **WHEN** startup finds an inactive interval with abandoned temporary chunks
- **THEN** it removes the verified owned chunks without following links outside the cache
- **AND** unfinished text remains available for recovery or explicit discard

### Requirement: Publish each stopped interval

Stopping live transcription SHALL seal the input at a source-frame boundary and start a bounded drain. The worker SHALL publish the resulting UTF-8 `.txt` automatically to the selected output folder. Publication SHALL use a unique name and an atomic, same-volume final rename. A failure affecting text SHALL leave independent recording operational.

#### Scenario: Read the final text without the application

- **WHEN** a user opens the published TXT
- **THEN** each speech entry identifies its source and available source time or labeled chunk range
- **AND** an available speaker label appears separately from the recognized words
- **AND** status and gap entries are visibly distinct from speech
- **AND** the session index maps these entries to stable source and segment identities

#### Scenario: Stop transcription while recording continues

- **WHEN** the user stops live transcription before stopping recording
- **THEN** the worker rejects later frames and finalizes all accepted ranges or records their incomplete outcomes by the drain deadline
- **AND** it publishes the text with the available source labels and transcript interval
- **AND** the session links intersecting recording spans by stable ID, including an audio file that finishes later
- **AND** starting live transcription again creates a distinct interval and output file

#### Scenario: Stop recording first or use transcription alone

- **WHEN** recording finishes before transcription, or the user never starts recording
- **THEN** live transcription continues until its own stop boundary
- **AND** final text can stand alone without a saved audio file
- **AND** playback links cover only source ranges present in finalized recordings

#### Scenario: Publish an incomplete or empty interval

- **WHEN** drain expires, the endpoint fails, or the interval contains no recognized speech
- **THEN** final text distinguishes unavailable coverage from an interval with no detected speech
- **AND** the session index reports completion status, interval bounds, and gaps
- **AND** a reply arriving after publication cannot append words to the published file

#### Scenario: Cancel pending transfers during drain

- **WHEN** the user cancels the remaining work during finalization
- **THEN** the worker starts zero further requests and attempts to cancel in-flight requests
- **AND** it records unknown remote outcomes and publishes the available committed text with the affected ranges

#### Scenario: An output path fails

- **WHEN** storage fills, a destination becomes unavailable, or another file already owns the requested name
- **THEN** the worker preserves existing files and its last durable progress
- **AND** the UI reports the actual cache or final location and any uncommitted coverage
- **AND** the user can retry local publication to a writable destination without repeating completed requests
