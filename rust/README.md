# Audio Capture for Windows

Use Audio Capture on Windows 11 x64 to record program audio, system audio, and one microphone. The recorder saves files locally. Start the companion worker when you want to transcribe completed files.

## Open the portable package

1. Extract the ZIP to a folder you can write to.
2. Keep `audio-capture.exe` and `transcribe.exe` in the same folder.
3. Run `audio-capture.exe`.

The ZIP includes this guide and dependency notices at `licenses/DEPENDENCIES.md`. Windows N editions can require the Media Feature Pack for MP3 encoding. FFmpeg and ffprobe are optional tools for transcription.

## Record a Discord conversation

1. Open Discord and join the call.
2. Select **Refresh** in Audio Capture.
3. Select the Discord process in **Programs**.
4. Select your input device in **Microphone**.
5. Select **Speech (Opus 32 kbps mono)** in **Recording profile**.
6. Set **Recording mode** to **Separate files**.
7. Leave **Skip silence** clear to retain quiet intervals.
8. Select your **Output folder**. The default is `Documents\AudioCaptures` under your Windows user folder.
9. Select **Start recording**.
10. Check that the Discord and microphone meters respond when people speak.
11. Select **Stop** to finalize the recordings.

Select **Separate files** for one track per program plus your microphone. Discord's output track can contain several participants. **Combined file** saves one mix. **Both** saves the mix and the separate files.

Process capture includes the selected process and its children. If Discord is missing from the list, clear **Show only programs with active audio** and refresh. Select the Discord process that carries the call audio. If Discord restarts, refresh and select the new process.

Use **Source levels** to check clipping and stereo cancellation during mono recording. Adjust **Program volume** or **Microphone volume** if necessary. **Add marker** adds a numbered point marker. The first marker creates an Audacity-compatible `Markers-<session>.labels.txt` file beside the recordings.

All separate tracks share the same saved timeline. **Pause** removes time from every track. **Skip silence** removes shared quiet intervals. Markers use the saved timeline, so pause or silence removal shifts them relative to the wall clock.

### Other sources and controls

- Use Ctrl-click to select multiple programs. Several browser tabs can share the same process tree.
- **Capture all system audio** records the default playback endpoint. For intentional system-plus-program overlap, select **Separate files** and leave **Monitor output** unset.
- Choose a different playback device in **Monitor output** when capturing system audio. **Monitor only** routes audio and creates no recording.
- Minimize the window to the notification area. Select the Audio Capture icon to restore it.
- Enter a name in **Saved setup**. Select **Save setup** to store your current sources and recording settings. Use **Load setup** to reuse them.
- After a device change, refresh and reselect an available microphone. If several running processes match a saved setup, select the intended process.

## Choose a recording profile

On first launch without saved or imported settings, Audio Capture starts with **Speech (Opus 32 kbps mono)**. It restores saved settings from `%LOCALAPPDATA%\AudioCaptureRust` when available. It can also read supported preferences and named presets from the original C++ recorder.

**Recording profile** sets the format, channels, and bitrate or sample rate for that format. Change the format controls directly to use other settings. The selector shows **Custom** when your encoding settings match none of the six profiles.

| Recording profile | Use and tradeoff |
| --- | --- |
| **Speech (Opus 24 kbps mono)** | Use for compact call recordings. The lower bitrate can lose speech detail. |
| **Speech (Opus 32 kbps mono)** | Use for calls and transcription. It uses more space than regular 24 kbps Opus. In one four-profile test, its transcripts were closest to those from the original audio. |
| **Speech DTX (Opus 24 kbps mono)** | Use when storage matters most and the recording has long pauses. This experimental mode can lose quiet speech. |
| **Speech DTX (Opus 32 kbps mono)** | Use for calls with pauses when you want a higher bitrate than 24 kbps DTX. This experimental mode can still lose quiet speech. |
| **General audio (Opus 96 kbps stereo)** | Use for music and other sources that need stereo. Opus uses lossy compression. |
| **PCM reference (WAV 16 kHz mono)** | Use when a speech service needs uncompressed 16-bit PCM. It takes more space than speech Opus and saves one channel. |

Leave **Skip silence** clear to retain quiet intervals on the saved timeline. DTX uses fewer bits during those intervals. Test a short sample with your speech and transcription service before you use DTX for a long recording.

For other WAV settings, choose 44100 Hz or 48000 Hz. **Auto** uses 48000 Hz. FLAC compresses PCM without loss. MP3 can add encoder delay and padding.

## Transcribe from Audio Capture

Have the API base URL and model name for your transcription service ready. The service must accept multipart requests at `<base-url>/audio/transcriptions`. For example, the base URL `http://127.0.0.1:8000/v1` uses `http://127.0.0.1:8000/v1/audio/transcriptions`.

1. Stop the recording and wait for Audio Capture to return to idle.
2. Select **Transcribe files…**.
3. Select the completed Discord and microphone files in **Select files to transcribe**. The picker opens at the recording output folder.
4. Enter the API base URL and model in the companion console. Use HTTPS for a remote service.
5. If the service requires a bearer token, enter the name of its environment variable. Leave the prompt blank for a service that needs no token.
6. Choose an output directory. Press Enter to use `transcripts` beside the first input.
7. Answer yes to the shared-timeline question for synchronized Discord and microphone tracks from the same recording.
8. Choose whether to request timestamps or split oversized files. Both choices default to no.
9. Choose whether to create a local HTML review. This choice defaults to yes and creates a new file in the transcript directory.
10. Check the inputs and destination. Type `y` at the final send prompt to proceed. Press Enter to cancel.

The console processes one file at a time and reports each result. Audio stays local until you confirm. The worker sends new jobs to the endpoint you enter. A loopback URL uses a service running on your computer. Press Enter when the console prompts you to close it.

You can also start the same guided console from PowerShell.

```powershell
.\transcribe.exe --guided -- '.\Discord.opus' '.\Microphone.opus'
```

### Set a bearer token for the session

If you need a token, open PowerShell in the extracted folder. Enter the token in the masked prompt. Start the recorder from that shell so its companion inherits the variable.

```powershell
$secret = Read-Host 'Transcription API key' -AsSecureString
$env:TRANSCRIPTION_API_KEY = [Net.NetworkCredential]::new('', $secret).Password
.\audio-capture.exe
```

Enter `TRANSCRIPTION_API_KEY` at the console's environment variable prompt. Run scripted commands that name this variable from the same shell. The worker reads the token from the environment and sends it as a bearer credential.

## Script transcription in PowerShell

Run these examples from the extracted folder. Replace the URL, model, and input paths with your service and recordings. The examples use a loopback service that accepts Opus. Use HTTPS for a hosted service.

```powershell
.\transcribe.exe --base-url 'http://127.0.0.1:8000/v1' --model 'local-asr' --output-dir '.\transcripts' -- '.\Discord.opus'
```

Append paths after `--` to queue multiple files. This example requests timed segments and creates a review with one timeline for the two tracks.

```powershell
.\transcribe.exe --base-url 'http://127.0.0.1:8000/v1' --model 'local-asr' --output-dir '.\transcripts' --same-session --timestamps --review '.\conversation.html' -- '.\Discord.opus' '.\Microphone.opus'
```

| Option | Effect |
| --- | --- |
| `--base-url URL` | Sets the API base URL, including any prefix such as `/v1`. Required. |
| `--model MODEL` | Selects the service's model or deployment name. Required. |
| `--output-dir DIR` | Sets the directory for text, available JSON, and job state. Required. |
| `--api-key-env NAME` | Reads the bearer token from a named environment variable. |
| `--language CODE` | Requests a language code supported by the service. |
| `--max-file-mib N` | Sets the per-file limit in MiB. The default is 25. Valid values are 1 to 1024. |
| `--timestamps` | Requests `verbose_json`. The service must support this response format for timed segments. |
| `--review PATH` | Creates an HTML review at a new file path. |
| `--same-session` | Groups the selected sources on one shared recording timeline. |
| `--chunk-long` | Uses FFmpeg to split audio that exceeds the configured limit. |
| `--retry-unknown` | Resends requests that may already have completed remotely. Check the service first. |
| `--allow-duplicate-sources` | Permits recognized combined and separate outputs from the same session in one batch. |
| `--allow-http` | Permits cleartext HTTP to a remote host. Loopback HTTP needs no flag. |
| `--help` | Shows the worker's command-line options. |

The service must accept the selected audio format. The worker accepts UTF-8 plain text or JSON with a string `text` field. Scripted mode starts eligible work when you run the command.

### File limits and FFmpeg

Use the service's actual upload limit. Guided mode uses the default 25 MiB limit. Set `--max-file-mib` in scripted mode when the service requires a different limit.

Put `ffmpeg.exe` on PATH to split oversized files with the guided option or `--chunk-long`. The worker reads the original audio and sends one bounded 16 kHz mono WAV part at a time. It keeps part offsets on the source timeline. Ordinary uploads within the configured limit need no FFmpeg.

Put `ffprobe.exe` on PATH to validate full-file segment times for HTML review. The text remains available when the service returns no usable timing or ffprobe cannot check the duration. Restart Audio Capture after an environment change so the recorder and its companion see the updated PATH.

### Review and resume

Open the generated HTML in your browser. Search the text, play or loop timed segments from their source files, and export corrections as text or JSON. Keep the original recordings at their current paths for playback. The review runs locally.

Use a shared timeline only for synchronized tracks from one recording. For unrelated recordings, leave the guided choice at no or omit `--same-session`. Each file then has its own timeline. When you record in **Both** mode, choose either the mixed file or its separate tracks to avoid duplicate speech.

To resume a batch, use the same output directory, source paths, audio, endpoint, model, and request options. Keep `.audio-capture-jobs` with the results. Choose a fresh HTML path when you request another review. Matching completed jobs reuse local results even when the endpoint is offline. An uncached job can contact the endpoint.

For a fully cached run, leave the key prompt blank or omit `--api-key-env`. If you specify a key variable, it must be set even if all jobs are cached.

A job with an unknown result may have completed on the server. Check the service before you use `--retry-unknown`. Keep the same output directory to retain completed work.

## Troubleshooting

- **Discord or microphone is quiet.** Check the selected process and input device. Refresh after a restart or device change. Check Windows microphone access and the two source meters.
- **A source drops out.** Check the recorder status. Other healthy sources can continue. Opus writes an Ogg checkpoint about once per second, which can leave a playable prefix after an interruption.
- **The transcription control is unavailable.** Stop the active recording or monitor session.
- **The companion console fails to start.** Check that `transcribe.exe` is beside the recorder.
- **The endpoint rejects the request.** Check the URL prefix, model name, token variable, media format, and upload limit. Use `--language` only with a code your service accepts.
- **A timed segment is missing.** Check that the endpoint supports `verbose_json` with segments and that ffprobe is on PATH for full-file review.
- **The review cannot play audio.** Check that the browser supports the format and the original audio remains at its recorded path.

## Terms and credit

This project is provided as-is for educational and personal use.

The Rust recorder follows the original AudioCapture project by masonasons. It uses the Win32 interface, WASAPI and Media Foundation, libopus, Ogg, and FLAC. [Original AudioCapture project](https://github.com/masonasons/AudioCapture)
