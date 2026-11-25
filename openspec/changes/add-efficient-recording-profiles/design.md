# Design

## Profile settings

Keep a built-in list of encoding choices in the preference model. Each entry specifies the format, channel count, and relevant bitrate or sample rate.

| Use | Format | Channels | Encoding setting |
| --- | --- | --- | --- |
| Speech | Regular Opus | Mono | 24 kbps |
| Speech | Regular Opus | Mono | 32 kbps |
| Speech with DTX | VOIP Opus with DTX | Mono | 24 kbps |
| Speech with DTX | VOIP Opus with DTX | Mono | 32 kbps |
| General audio | Regular Opus | Stereo | 96 kbps |
| PCM reference | WAV | Mono | 16 kHz |

Use the existing encoder paths. Regular Opus uses its existing audio application mode. DTX uses the existing VOIP mode. DTX uses less data in quiet intervals and retains their time. The separate **Skip silence** option can remove time.

An encoding-only profile lets the user retain their capture setup. A full preset would apply saved routing and folder values as well. Keep the full-preset path for users who want to restore those values.

## Preference compatibility

Keep the historical settings defaults for fields absent from version 1 files or legacy imports. If neither current nor legacy active settings exist, apply regular 32 kbps mono Opus. Open existing settings with their saved or historical values.

Use the existing preference version and serialized fields. Store the effective settings when the user saves. Derive the current profile from those settings each time the interface updates. A match compares the format, channel count, and active format setting. Other codec settings and capture settings do not affect the result.

## Windows interface

Show a read-only **Recording profile** selector and short usage guidance with the manual encoding controls. Show **Custom** when no profile matches. Selecting **Custom** leaves the manual settings in place.

Applying an encoding profile changes the format, channel count, and active format setting. Preserve input identities, input filtering, output mode, folder, gains, recording and monitor routes, monitor-only mode, and silence and pause behavior.

Keep named **Saved setup** controls separate. A saved setup still stores and restores all settings. Refresh the profile display after loading a setup or changing a codec control. Follow the existing idle-state policy for edits. Include the new controls in keyboard navigation and layout checks.

## Initial profile decision

The comparison used the application and microphone tracks from one 29-second call. Each original was already compressed with Opus. The original-track recognition serves as the comparison baseline.

The harness decoded each original once to 48 kHz mono PCM. Each variant received identical samples for its track. The harness used the recorder's [regular and DTX encoder paths](../../../rust/src/opus_writer.rs).

The same resampling step produced 16 kHz mono input for all ten ASR runs. Every variant retained 1,396,203 decoded samples per track at 48 kHz.

The offline recognizer used English `base.en`, faster-whisper 1.2.1, CTranslate2 4.7.2, CPU int8, and four threads. Every fixed run used beam size five, voice activity detection, zero temperature, and independent text for each segment.

| Candidate | Total bytes for both tracks |
| --- | ---: |
| Regular Opus, 24 kbps | 119,546 |
| Regular Opus, 32 kbps | 157,593 |
| Opus with DTX, 24 kbps | 53,271 |
| Opus with DTX, 32 kbps | 69,173 |

Prefer the smallest candidate that retains decoded timing and avoids material changes to the original-track recognition. Both 24 kbps variants produced word losses or substitutions. The 32 kbps DTX variant used 56.1% fewer bytes than regular 32 kbps and produced a larger substitution on the application track.

Choose regular 32 kbps mono Opus for first run. It preserved decoded timing and had the closest transcript agreement among the four candidates. It was the smallest candidate that met the recognition criterion in this comparison.

The call had no independently verified transcript. The results measure recognition agreement after re-encoding existing Opus audio. First-generation capture quality, listening quality, and other recording conditions remain unmeasured.

The private evaluation output contains exact transcript comparisons and the audio or references needed for review. Repository documentation records the method, bounded results, and limitations. Use generated or otherwise public material in examples and packaged assets.

## Verification

Exercise the historical, first-run, and named setup paths with independent preference fixtures. Verify each encoding profile and confirm that application changes only its encoding settings.

Run the focused preference, Opus, layout, and Windows recorder checks. Exercise the packaged recorder with a clean preference location. Change a profile, edit the bitrate, and load a saved setup. Check the selected profile and the settings after each action.

Review the user guide and wiki against the implemented controls and source links. Check the distributable file list for private audio, exact transcript differences, and local-only paths.
