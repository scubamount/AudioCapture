# Performance on Windows 11 x64

The recorder uses native Windows controls and WASAPI. Recording does not start the transcription worker or load a speech model. Opening the guided workflow starts a separate process. Long-file conversion and the optional local diarizer can use FFmpeg. The worker can use ffprobe to verify timestamps.

The speaker-enabled Windows ZIP is 2,035,192 bytes. It contains a 1,629,696-byte recorder, a 2,522,112-byte worker, and a 5,682-byte Python adapter, plus the guide and notices. Install the optional Python runtime and models separately. The previous speech-profile ZIP was 1,344,067 bytes and had the same recorder size.

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

### Speaker attribution

The test used 24 three-voice calls with 602.29 seconds of synthetic audio. Each call had a mixed version and separate program and microphone tracks. Twelve or more calls contained overlapping speech. The cases rotate three Windows voices across ten scripts. The worker received reference recognition text and timing. Speech recognition errors were outside this test. The fixture generator and scorer are available in the [acceptance tool](../rust/tools/test_speaker_acceptance.py).

The table compares two offline model configurations. Both used sherpa-onnx 2.0.3 and the same Pyannote 3.0 segmentation model. The targets were at most 20% diarization error and at least 90% recall on isolated reference turns.

| Embedding model and clustering threshold | Source layout | Diarization error | Isolated turn recall | Error during overlap |
| --- | --- | ---: | ---: | ---: |
| NeMo TitaNet Small, 0.50 | Separate tracks | 7.66% | 91.82% | 0.00% |
| NeMo TitaNet Small, 0.50 | Single mix | 21.38% | 79.09% | 45.15% |
| WeSpeaker ResNet34-LM, 0.27 | Separate tracks | 7.26% | 92.73% | 0.00% |
| WeSpeaker ResNet34-LM, 0.27 | Single mix | 23.60% | 81.82% | 67.05% |

Both configurations met the targets on separate tracks. Both missed them on the mix. Much of the overlap in the separate recordings crossed the microphone and program tracks. These results do not establish accuracy for multiple simultaneous voices on one Discord track.

The TitaNet adapter processed the mixed set in 50.77 seconds and the separate tracks in 74.11 seconds. Peak sampled process-tree working sets were 253.5 MiB and 237.7 MiB, respectively. The adapter exits after use. Ordinary recording does not incur that memory cost.

One private local check used two 29.088-second recordings from the same call. The worker retained all 15 recognized words. Twelve had sufficient speaker evidence. Three remained unassigned. Both speakers stayed anonymous. Original audio and transcript text are excluded from the repository.

#### Automatic names

The same 24-call corpus tested explicit introductions and corroborated exchanges. The tests included 48 misleading utterance instances and four conflicting scenarios. The table lists names displayed automatically. A suggestion that awaited review counts as an abstention.

| Speaker evidence | Introductions recognized | Contextual names recognized | Incorrect named turns / named turns |
| --- | ---: | ---: | ---: |
| Reference speaker labels | 22 / 23 | 21 / 21 | 0 / 130 |
| Local TitaNet, separate tracks | 15 / 23 | 15 / 21 | 0 / 90 |
| Local TitaNet, single mix | 0 / 23 | 0 / 21 | 0 / 0 |

On the single mix, local acoustic results generated suggestions only. The zero count represents abstention. None of these runs automatically confirmed a cross-session identity. Naming rules were corrected against failed cases, so the final naming scores are regression results on fixed scripts.

A further three-person dialogue exercised paraphrased responses and an introduction followed by another sentence. Provider-labeled turns supported two inferred names; removing approved context left one. Local mixed-audio results displayed suggestions and attributed six of twelve turns uniquely and correctly. This was a synthetic workflow check with supplied recognition text.

### Batch transcription

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
