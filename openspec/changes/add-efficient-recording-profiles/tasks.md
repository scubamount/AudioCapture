# Implementation tasks

## 1. Evaluate speech encodings

- [x] 1.1 Transcribe the previous take's original tracks and four controlled Opus variants with one offline ASR configuration.
- [x] 1.2 Compare bytes, decoded timing, and transcript differences across the same tracks. Keep the exact transcript differences in private output.
- [x] 1.3 Record the initial profile decision and the evidence limits in this change.

## 2. Implement preferences and controls

- [x] 2.1 Add six immutable encoding profiles and test profile application and matching with inactive codec fields.
- [x] 2.2 Use the selected profile for clean first-run settings. Test version 1 partial settings, full saved setups, and legacy preferences.
- [x] 2.3 Show the Windows profile selector and matching or **Custom** state. Keep codec controls and named saved setups available.
- [x] 2.4 Test that profile application preserves sources, routes, output mode, folder, gains, monitoring, and silence and pause settings.

## 3. Document and verify delivery

- [x] 3.1 Update the guide and write the source-linked LLM wiki. Document first-run behavior, profile uses, DTX, and the scope of the comparison.
- [x] 3.2 Check screenshots, fixtures, examples, and packaged files for private content and local-only references.
- [x] 3.3 Run strict OpenSpec validation.
- [x] 3.4 Run the focused Rust checks and record the clean-settings Windows smoke result.

## Verification

Eleven focused release preference tests passed. The profile tests compared the full settings before and after applying each built-in. The compatibility tests used separate current and legacy settings fixtures.

Four focused release Opus writer tests passed. They covered decoded duration, DTX storage, protection of existing files, and the recovered prefix after interruption.

Nine focused release native controller and layout tests passed. The full focused run recorded 100 passing checks across the recorder, worker, and library. Two opt-in checks were ignored.

Four packaging tests passed. Strict validation passed for all seven OpenSpec changes. The privacy review covered the source, generated assets, and portable package contents.

The packaged Windows recorder started with regular 32 kbps mono Opus in a clean settings location. All six profiles displayed their expected format, channel count, and rate.

Changing mono WAV to 44.1 kHz displayed **Custom** and 705.6 kbps. Loading the saved setup restored **Custom**. A profile change kept the current folder edit.

The regular Opus bitrate remained visible at window widths of 660 and 390 pixels. Both closed DTX bitrate selectors fit at 390 pixels. Changing the DTX bitrate from 24 to 32 selected the matching 32 kbps DTX profile.

Native layout tests covered scaling. Secondary-display DPI on hardware was not checked.
