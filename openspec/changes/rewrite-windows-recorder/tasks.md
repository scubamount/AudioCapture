# Implementation tasks

## 1. Establish the baseline

- [x] 1.1 Add a failing layout regression for the reported Opus view and its conditional controls.
- [x] 1.2 Record upstream build identity, supported workflows, control bounds, and scaling for the comparison.
- [x] 1.3 Resolve the first-release OS range and WAV sample-rate choices against the reviewed specification.

## 2. Implement the recorder

- [x] 2.1 Implement process discovery, active-session filtering, microphone selection, and explicit system capture.
- [x] 2.2 Implement bounded capture queues, timestamp alignment, resampling, gains, and monitor output.
- [x] 2.3 Implement the four encoders and the three file modes with collision-safe filenames.
- [x] 2.4 Implement pause, resume, stop, silence handling, progress reporting, and partial-failure recovery.
- [x] 2.5 Implement the responsive UI with accessible controls, focus handling, and tray restore.
- [x] 2.6 Implement settings persistence and read-only import of legacy presets.

## 3. Verify on Windows

- [x] 3.1 Run the layout matrix and capture the Opus regression result at the reported size.
- [ ] 3.2 Test two distinguishable app signals and a microphone across two consecutive sessions. Decode outputs and check isolation.
- [x] 3.3 Verify file counts, codecs, audio rates, time alignment, pause transitions, and monitor-only behavior.
- [ ] 3.4 Exercise source loss, unavailable encoders, output write failure, and missing preset devices.
- [x] 3.5 Open the mixed WAV in a transcription tool and verify both synthetic spoken phrases.
- [ ] 3.6 Check a portable release build on a clean Windows machine. Measure resource use and document redistributed components.
## Verification limits

The Windows 11 host passed the two-process isolation and mixed-track tests. The microphone endpoint was enumerated. The tests used generated audio. Physical microphone capture and device removal still need a hardware check.

The tests exercised source exit, existing-file collisions, monitor disconnection, malformed settings, and missing identities. A Windows installation without Media Foundation and a real disk-full event remain untested.

The release passed on the build host. Its direct DLL imports were inspected. A clean Windows installation remains to be tested. See the [Rust recorder guide](../../../rust/README.md) for build and verification commands.
