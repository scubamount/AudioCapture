# Implementation tasks

## 1. Establish regression fixtures

- [ ] 1.1 Pin the baseline revision and toolchain. Record fixture generation and output hashes.
- [x] 1.2 Add a subprocess test that interrupts a 32 kbps mono Opus writer after four seconds of accepted synthetic audio.
- [x] 1.3 Add two-source tests for wholly late and partially late packets. Include a healthy source that continues after the late packet.
- [x] 1.4 Add impulse fixtures for both ends of a one-second WAV. Include 16 and 44.1 kHz outputs and writes across chunk and rollover boundaries.

## 2. Protect recordings

- [x] 2.1 Implement periodic complete Ogg pages and file-buffer flushes. Checkpoint by one second of active input.
- [ ] 2.2 Verify interruption recovery within the stated frame and lookahead bound. Verify normal stop and flush errors.
- [x] 2.3 Trim late committed frames and place usable suffixes at their original timestamps. Report per-source loss.
- [ ] 2.4 Verify healthy tracks, silence gaps, pause alignment, malformed frames, future timestamps, and bounded queues.
- [x] 2.5 Reject known overlaps in mixed and monitored modes. Preserve intentional separate-only tracks.
- [ ] 2.6 Compare the actual pinned capture and monitor endpoint identities. Block matching or indeterminate routes. Test endpoint changes without playing live feedback.
- [ ] 2.7 Handle and count WASAPI discontinuities and unreliable timestamps. Verify one healthy source continues and uncertain loss is reported.
- [x] 2.8 Keep healthy sources running after another source exits. Stop and report loss when the last source exits.
- [ ] 2.9 Verify fatal writer errors terminate without retrying or duplicating the failed block.

## 3. Improve recording usability

- [x] 3.1 Add per-source peak and recent-activity indicators with accessible names and bounded UI updates.
- [ ] 3.2 Verify active speech, quiet audio, silence, pause, source exit, resizing, and keyboard access.
- [x] 3.3 Clarify separate-track and compact-speech choices. Keep current application defaults, the 32 kbps mono workflow, and all existing modes.
- [ ] 3.4 Show pre-gain activity, retained clipping, and mono cancellation. Test equal, opposing, one-sided, quiet, and clipped signals.
- [ ] 3.5 Write explicit session markers to an Audacity label sidecar. Test pause, skipped silence, collisions, and write failures.
- [x] 3.6 Verify marker queue locks are released before audio rendering, encoder writes, and sidecar flushes.

## 4. Preserve output and preferences

- [x] 4.1 Flush and compensate the WAV resampler tail. Verify sample counts, impulses, alignment, and rollover.
- [ ] 4.2 Preserve exact legacy gain until edited or expose 1% increments. Test import, start, save, and close.
- [ ] 4.3 Separate process enumeration from active-session detection. Test an endpoint failure and a disappearing session.
- [ ] 4.4 Bind packaged executables and notices to the same locked source build. Test a changed source and a changed lockfile.

## 5. Measure and optimize

- [x] 5.1 Replace oversized retained Opus packet buffers. Check decoded audio, timing, final granule, and interruption recovery.
- [x] 5.2 Consume pause transitions incrementally. Verify shared timing with repeated pauses and silence skipping.
- [x] 5.3 Run the isolated timeline benchmark and checkpoint-matched allocator harness before and after. Record fixture, flags, platform, output size, duration, and peak Rust heap bytes. Verify that peak Rust heap bytes decrease and decoded PCM and timing remain equivalent.
- [ ] 5.4 Measure checkpoint overhead and a real one-minute app-plus-microphone session. Report CPU and file size by output mode.
- [ ] 5.5 Run the focused tests in the design. Record failed checks and remaining hardware and power-loss limits.
- [x] 5.6 Add an opt-in mono Opus speech profile using VOIP and DTX. Preserve the selectable bitrate, ordinary Opus mode, and defaults.
- [ ] 5.7 Evaluate Matroska track timing and process-tree exclusion before considering either option for production.
- [ ] 5.8 Verify speech-profile sample count, quality limitations, saved settings, pause alignment, and interrupted recovery. Evaluate representative real-call speech and music separately.

## Verification

The final focused release run passed 85 Rust tests across recording, encoders, settings, the Windows interface, and the transcription companion. Four packaging regressions also passed. An interrupted Opus stream lost 32.77 ms in the synthetic fixture. Its recovered PCM matched the corresponding portion of the finalized stream.

Two Windows process-loopback tests exercised selected processes and decoded their output. A second tone remained audible in its recorded track after the first process exited. The native GUI showed the Opus bitrate and WAV sample-rate selectors and wrote an in-range marker. Layout tests covered scaling; physical hot-unplug and power-loss tests remain open.

At 32 kbps, the optional DTX profile preserved all 2,550,013 samples of each speech fixture. It reduced file size by 48% and 62%. The recognizer misheard a term on the noisy variant. Further testing covers real calls, music, and hardware timing faults. A forced checkpoint flush error, real disk exhaustion, and a clean Windows installation remain untested.
