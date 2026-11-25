# Recording

[Wiki home](README.md) · [Operator guide](../../rust/README.md) · [Focused checks](verification.md)

## Capture and routing

| Stage | Code and behavior |
| --- | --- |
| Window and layout | [`native_window.rs`](../../rust/src/native_window.rs) owns Win32 controls and events. [`native_layout.rs`](../../rust/src/native_layout.rs) places visible controls at the current width and DPI. |
| Selection | [`Controller::start`](../../rust/src/native_controller.rs) checks selected devices and process identities. [`sources.rs`](../../rust/src/sources.rs) enumerates devices, processes, and active sessions. |
| Route validation | [`validate_routes`](../../rust/src/sources.rs) pins the system render endpoint before capture. It rejects known duplicate sources in a mix or monitor route. It rejects monitoring system audio through the same render endpoint. Separate-only output permits intentional source overlap. |
| WASAPI input | [`capture.rs`](../../rust/src/capture.rs) captures a process tree, the system render endpoint, or the microphone. Each input produces timestamped 48 kHz stereo float packets. |
| Timeline and mix | [`Session` and `render_block`](../../rust/src/record.rs) align packets, apply gain, and write the selected outputs. Mix and separate tracks share the same retained frames. |
| Monitor and markers | [`Monitor`](../../rust/src/monitor.rs) sends audio to the selected playback endpoint. Monitor-only sessions create zero recording files. [`MarkerWriter`](../../rust/src/markers.rs) writes Audacity point labels after the first marker. |

The mixer keeps a usable late packet suffix at its original time. It reports frames that arrived after their output position. After startup, a healthy source can continue if another source exits. A monitor error can end playback while recording continues. A fatal audio writer error ends the recording interval. An active transcript can continue on healthy sources. [Recorder session](../../rust/src/record.rs)

Pause and silence skipping remove the same intervals from every saved track. Markers follow the saved timeline. Blocks that retain all frames skip the per-frame mask. The capture and monitor queues have fixed capacities. Worker limits and bounded stop waits protect repeated starts and stops. [Recorder session](../../rust/src/record.rs) · [Monitor](../../rust/src/monitor.rs)

## Independent output intervals

The controller shares one acquisition across active recording and live transcription. Each interval has a start and end on the session's 48 kHz source clock. **Stop both** gives both intervals one cutoff.

The live tap receives per-source audio before recording pause, shared silence removal, or encoding. The recorder can stop accepting transcript audio without waiting for recognition or text publication. [Capture session](../../rust/src/record.rs) · [Wire protocol](../../rust/src/live_wire.rs)

An audio interval that overlaps live transcription writes `Recording-<id>.timeline.jsonl`. Version 1 lists output files and codec timing precision. When transcription joins an existing recording, `mapped_from` and `saved_at_mapped_from` identify where mapping starts. Earlier cuts have no map. Mapping continues until recording stops. [Timeline writer](../../rust/src/record/intervals.rs)

A mapped span starts and ends on the session clock. Its saved offset uses 48 kHz frames before output resampling. Use the file's output rate to calculate a PCM seek position. Compressed formats have codec-dependent seek precision. [Timing specification](../../openspec/changes/add-independent-live-transcription/specs/silence-and-time-mapping/spec.md)

**Pause** and **Skip silence** can remove frames from a saved output. A word in a removed range keeps its session time. A word outside the recording interval also keeps its session time. These words have no saved audio position.

## File output

The session supports separate files, one mix, or both. [`Writer`](../../rust/src/encode.rs) streams stereo frames to the selected encoder and applies the output channel count. Writers preserve existing destination files.

| Format | Output behavior |
| --- | --- |
| WAV | 16-bit PCM at 16, 44.1, or 48 kHz. Auto selects 48 kHz. The WAV writer preserves resampler state across writes and rollover. It flushes the filter tail on finish. [WAV writer](../../rust/src/encode.rs) |
| MP3 | Windows Media Foundation at 48 kHz. The encoder can add delay and padding. [MP3 writer](../../rust/src/mp3_writer.rs) |
| Opus | Ogg Opus at 48 kHz. The writer flushes an Ogg page after each second of encoded frames. A process interruption can leave a playable prefix. [Opus writer](../../rust/src/opus_writer.rs) |
| Opus speech DTX | Optional mono Opus with the selected bitrate and the standard `.opus` extension. DTX keeps the timeline when silence skipping is off. [Opus writer](../../rust/src/opus_writer.rs) · [Session filenames](../../rust/src/record.rs) |
| FLAC | Lossless PCM compression at 48 kHz with a selectable compression level. [FLAC writer](../../rust/src/flac_writer.rs) |

## Recording profiles and saved setups

The **Recording profile** selector applies the format, channels, and that format's rate or bitrate. It retains settings for other codecs, selected sources, routes, gains, folder, file mode, and silence setting. Manual edits and setup loads refresh the match. **Custom** shows an encoding outside the built-in choices. [`ENCODING_PROFILES`](../../rust/src/prefs.rs) · [Profile selection](../../rust/src/native_controller.rs) · [Operator profile table](../../rust/README.md#choose-a-recording-profile)

Use **Saved setup** to store or restore the full configuration. The clean-start encoding is **Speech (Opus 32 kbps mono)** when both current and legacy active settings are absent. Existing version 1 files and supported legacy imports use historical defaults for missing fields. [Settings loader and defaults](../../rust/src/prefs.rs) · [Profile specification](../../openspec/changes/add-efficient-recording-profiles/specs/recorder-preferences/spec.md)

[`prefs::Settings`](../../rust/src/prefs.rs) defines defaults and valid settings. The Rust recorder writes versioned JSON to `%LOCALAPPDATA%\AudioCaptureRust`. It uses `settings.json` for current preferences and `presets/<name>.json` for named presets.

Current Rust data takes priority. If a current file is absent, the recorder can read the corresponding legacy JSON under `%LOCALAPPDATA%\AudioCapture`. It reads legacy files in place. Saving writes Rust data. Deleting a legacy preset hides it through a Rust-side entry. [Preset load, save, and delete](../../rust/src/prefs.rs) · [Compatibility specification](../../openspec/changes/rewrite-windows-recorder/specs/recorder-preferences/spec.md)

Presets store program names and endpoint IDs. The controller restores a program when exactly one current process matches and its identity is available. It tracks live selections by PID and creation time. Missing or ambiguous selections require user action before capture. Legacy presets with multiple microphones require the user to choose one. Legacy monitor indices require a fresh endpoint selection. [Controller selection](../../rust/src/native_controller.rs) · [Legacy import](../../rust/src/prefs.rs)

Recent programs use a bounded `recent-programs.json` history in the same settings directory. The recorder checks the executable identity against a discovery snapshot before applying a recent match. Automatic recent selection is opt-in and requires a unique match. [Recent history](../../rust/src/recent_programs.rs) · [Matching requirements](../../openspec/changes/add-independent-live-transcription/specs/remembered-programs/spec.md)

## Profile comparison

Regular 32 kbps mono Opus stayed closest to the source recognition results in a short offline trial. The DTX variants used less space and changed some recognized speech. The trial reused lossy sources and had no independent reference transcript. See the sample, candidate sizes, timing, and ASR settings in the [measurement receipt](../performance.md#size-and-speech-recognition).
