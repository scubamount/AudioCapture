# Performance on Windows 11 x64

The recorder uses native Windows controls and WASAPI. Recording does not start the transcription worker or load a speech model. Opening the guided workflow starts a separate process. FFmpeg is used only for explicitly requested long-file conversion. The worker can also use ffprobe to verify review timestamps.

The Windows ZIP is 1,344,067 bytes. It contains a 1,629,696-byte recorder and a 781,312-byte transcription worker. The guide and dependency notices account for the remaining archive entries. The six profiles added 2,231 bytes to the ZIP compared with the earlier integrated build.

## Idle window

Before UI automation attached, the speech-profile build used 3,551,232 private bytes and a 19,902,464-byte working set. Its process CPU time did not increase during the 10.004-second idle sample. We did not check which window had focus.

An earlier integrated build used 3.86 MiB of private memory and a 21.41 MiB working set after startup on one Windows 11 PC. Its measured CPU time did not increase during a 10.002-second sample before desktop inspection. This short measurement excludes recording and transcription.

Active desktop accessibility inspection produced a different workload. An instrumented window used about five CPU-seconds per ten seconds while inspected. Windows accessibility requests accounted for 44,749 of 50,000 traced window events. The trace recorded two layouts and no recorder timer callbacks. Foreground idle CPU without inspection and CPU with other accessibility clients were not checked.

## Recording

For a block with every audio frame retained, the mixer now tracks the frame count directly. Only a partially retained block needs a per-frame mask. Markers still use positions in the saved audio.

A release benchmark mixed 10,000 two-source blocks with no pauses or skipped silence. The median fell from 14.63 ms to 13.43 ms across 25 alternating pairs on one logical CPU. This benchmark creates fresh scratch storage per block. It excludes device capture, encoding, and marker commits.

### Size and speech recognition

We decoded two simultaneous 29.088-second speech tracks. The Rust writer encoded each track four ways from the same samples. We resampled every version to 16 kHz for recognition.

Local faster-whisper 1.2.1 used `base.en`, English, CPU int8, four threads, beam size five, zero temperature, and voice activity detection. Previous-text conditioning was off.

| Mono Opus profile | Combined size for both tracks | Agreement with the original files' transcripts |
| --- | ---: | --- |
| Regular, 32 kbps | 157,593 bytes | One word changed on the program track. One word disappeared on the microphone track. |
| Speech DTX, 32 kbps | 69,173 bytes | The short program reply changed completely. The microphone transcript matched. |
| Regular, 24 kbps | 119,546 bytes | The program reply changed completely. One microphone word disappeared. |
| Speech DTX, 24 kbps | 53,271 bytes | The program reply changed in part. The microphone transcript lost its closing phrase. |

Regular 32 kbps had the closest overall match. The recorder uses it for new settings. Speech DTX at 24 kbps produced the smallest files.

Both original files already used Opus. The test measured a second encoding pass. No independent transcript was available. All variants retained all 1,396,203 samples per track. Speech DTX changes the Opus application to VoIP and enables discontinuous transmission. This comparison measures both changes together.

### Opus input processing

We tested copying samples in larger groups before Opus encoding. Each configuration used ten alternating release-build pairs on a 12-second fixture.

| Encoding | Median paired change in elapsed time |
| --- | ---: |
| Speech DTX, 24 kbps mono | −1.00% |
| Regular Opus, 32 kbps mono | +1.25% |
| Regular Opus, 128 kbps stereo | −0.15% |

The final files and sampled checkpoints matched byte for byte. The recorder retains its existing input loop.

## Transcription

The worker checks the size limit from file metadata. It can reject oversized audio before reading its contents. Eligible files still use a content hash for reliable resume.

The previous worker read a 12 KiB saved-response fixture one byte at a time. Reading the same fixture through a buffer reduced 12,327 reader calls to at most three. In a separate Windows process test, replaying a 98,315-byte JSON response used 20 read operations, down from 99,109. Both versions reused the saved response locally.

The packaged worker processed a two-hour synthetic 16 kHz source with a local mock endpoint. It sent nine uploads using a 25 MiB file limit and created one temporary WAV at a time. Peak working sets were 10.39 MiB for the worker and 23.71 MiB for FFmpeg. A completed repeat took 0.159 seconds and made zero uploads with FFmpeg unavailable. The mock measured batch processing overhead and excluded recognition time.

## Repeat the focused checks

Run these commands from the repository's `rust` directory using the build prerequisites in the [build guide](../BUILD.md).

```powershell
cargo test --release --locked --lib unpaused_tail_keeps_pcm_without_allocating_a_mask
cargo test --release --locked --lib benchmark_timeline -- --ignored --nocapture
cargo test --release --locked --bin transcribe cached_response_json_uses_batched_reads
cargo test --release --locked --bin transcribe rejects_oversized_input_without_reading_locked_contents
```

Use the same build flags, machine, and power settings when comparing timings. For recording size and speech-quality examples, see the [operator guide](../rust/README.md).
