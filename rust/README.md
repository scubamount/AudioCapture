# Audio Capture 2.2.0 for Windows

Record selected program audio, system audio, and one microphone on Windows 11 x64. Transcribe finished English MP4s offline or use a compatible endpoint for live audio and other completed files.

| Task | Guide |
| --- | --- |
| Record only | [Record a Discord conversation](#record-a-discord-conversation) · [Choose a recording profile](#choose-a-recording-profile) |
| Turn one MP4 into timed text offline | [Transcribe an MP4 offline](#transcribe-an-mp4-offline) |
| Submit an offline MP4 from an agent | [Control a local MP4 job](#control-a-local-mp4-job) |
| Use a configured service | [Transcribe during a call](#transcribe-during-a-call) · [Use an endpoint for completed files](#use-an-endpoint-for-completed-files) |
| Attribute speakers and export approved text | [Review speakers and export memory](#review-speakers-and-export-memory) |

## Open the portable package

1. Download the app ZIP from [Audio Capture 2.2.0](https://github.com/scubamount/AudioCapture/releases/tag/v2.2.0).
2. Compare its SHA-256 against the row for the app ZIP in `SHA256SUMS.txt`.
3. Extract the ZIP to a folder you can write to.
4. Keep `audio-capture.exe` and `transcribe.exe` in the same folder.
5. Run `audio-capture.exe`.

The ZIP includes this guide, `config/transcription.example.json`, and dependency notices at `licenses/DEPENDENCIES.md`. Windows N editions can require the Media Feature Pack for recording and video import. [Recorder overview](https://github.com/scubamount/AudioCapture/blob/v2.2.0/docs/images/recorder-overview.png)

## Record a Discord conversation

1. Open Discord and join the call.
2. Select **Refresh** in Audio Capture.
3. Open **Select programs…** and select the Discord process.
4. Select your input device in **Microphone**.
5. Open **Recording settings** and choose **Speech (Opus 32 kbps mono)**.
6. Set **Recording mode** to **Separate files**.
7. Leave **Skip silence** clear to retain quiet intervals.
8. Select your **Output folder**. The default is `Documents\AudioCaptures` under your Windows user folder.
9. Select **Start recording**.
10. Check that the Discord and microphone meters respond when people speak.
11. Select **Stop recording** to finalize the recordings.

Select **Separate files** for one track per program plus your microphone. Discord's output track can contain several participants. **Combined file** saves one mix. **Both** saves the mix and the separate files.

Process capture includes the selected process and its children. If Discord is missing from the list, clear **Show only programs with active audio** and refresh. Select the Discord process that carries the call audio. If Discord restarts, refresh and select the new process.

Use **Source levels** to check clipping and stereo cancellation during mono recording. Adjust **Program volume** or **Microphone volume** if necessary. **Add marker** adds a numbered point marker. The first marker creates an Audacity-compatible `Markers-<session>.labels.txt` file beside the recordings.

All separate tracks share the same saved timeline. **Pause recording** removes time from every track. **Skip silence** removes shared quiet intervals. Markers use the saved timeline, so pause or silence removal shifts them relative to the wall clock.

### Other sources and controls

- Use Ctrl-click to select multiple programs. Several browser tabs can share the same process tree.
- **Capture all system audio** records the default playback endpoint. For intentional system-plus-program overlap, select **Separate files** and leave **Monitor output** unset.
- Choose a different playback device in **Monitor output** when capturing system audio. **Monitor only** routes audio and creates no recording.
- Minimize the window to the notification area. Select the Audio Capture icon to restore it.
- Enter a name in **Saved setup**. Select **Save setup** to store your current sources and recording settings. Use **Load setup** to reuse them.
- After a device change, refresh and reselect an available microphone. If several running processes match a saved setup, select the intended process.

### Use a recent program

The recorder remembers programs after a successful start. Choose an entry in **Recent programs** and select **Use recent**. This selects its current process and its saved microphone, when available. Check the selected sources before you start an activity.

Open **Manage recent** and select **Auto-select recent** if you usually capture the same program. The recorder can preselect a verified match when a single process tree matches. Select the source yourself when the executable has changed or the match is ambiguous. Use **Forget recent** for one entry or **Clear recent** for the list.

## Choose a recording profile

Open **Recording settings** to select a preset or adjust its format, channels, bitrate, and sample rate. Available controls depend on the selected format. [Recording settings screenshot](https://github.com/scubamount/AudioCapture/blob/v2.2.0/docs/images/recording-profiles.png)

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

## Transcribe during a call

Live transcription uses a compatible endpoint. Recording and live text have separate start and stop controls. [Live workflow and screenshot](https://github.com/scubamount/AudioCapture/blob/v2.2.0/docs/live-transcription.md)

1. Select the program and microphone for the transcript.
2. Choose your recording **Output folder** if you also want to save audio.
3. Open **Transcription settings**. Load a config or enter the API base URL and model.
4. Choose the **Transcript output folder**. Leave it empty to use the recording output folder.
5. If your service needs a bearer token, fill in **API key environment variable**.
6. Select **Start transcription**.
7. Check the confirmed sources, destination, model, and output path. Approve the interval to begin.
8. Select **Stop transcription** to finish the text file. Follow the status until the final path appears.

Select **Start recording** during a text-only call to also save audio. Select **Stop recording** to close the audio while transcription continues. Select **Stop both** to give both active intervals the same cutoff.

The worker sends short WAV chunks to your approved endpoint. Use HTTPS for a remote service. It writes committed text to a readable `.partial.txt` in the local cache. Select **Open transcript** to read the current text. The final TXT and its segment index go in the selected output folder. If recognition cannot complete a range, the result identifies missing coverage. A text-only interval saves no full-call audio.

**Pause recording** and **Skip silence** affect the saved recording. Live transcription uses source time and can cover a pause or an interval with no saved audio. Opus DTX reduces audio bytes during quiet intervals while preserving their duration when **Skip silence** is off.

### Load a transcription config

Open **Transcription settings** and select **Load config…**. Choose your JSON file. Use **Save config…** to copy the current fields to a file.

For a fresh extraction, put your file beside `audio-capture.exe` and name it `transcription.json`. The recorder loads it when it opens. Use **Load config…** for a file with any name or location. Each start still asks you to confirm the sources, endpoint, model, and output path.

The example in `config/transcription.example.json` uses OpenAI's `whisper-1` model. Copy the example before you edit it. Set `endpoint` and `model` for your service. Use these keys in a version 1 file.

| Key | Value |
| --- | --- |
| `version` | Use `1`. |
| `endpoint` | API base URL including its path prefix, for example `https://api.openai.com/v1`. |
| `model` | Model name understood by the service. |
| `api_key_env` | Name of the environment variable that contains the key. Leave it empty if the service needs no key. |
| `transcript_output_folder` | Destination for final text. An empty value uses the recording output folder. |
| `request_segment_timestamps` | Set to `true` when your service supports timed segments. |
| `speech_gating`, `speaker_labels` | Keep both at `false`. These features are in evaluation. |
| `speaker_adapter_config` | Optional path to a local adapter config. Leave it empty when you have no adapter. |

Relative folder and adapter paths resolve from the directory that contains the JSON file. Store API keys in environment variables and put only their names in the config.

Run this check from the extracted folder to validate the example without opening the window or contacting the endpoint.

```powershell
$check = Start-Process -FilePath .\audio-capture.exe -Wait -PassThru -ArgumentList @(
    '--check-transcription-config', '.\config\transcription.example.json'
)
$check.ExitCode
```

Exit code `0` means the file passed validation. Omit the path to check `transcription.json` beside the executable.

### Recover transcript text

If Windows closes the worker or the output folder fails, inspect the cached partial text. From the folder that contains `transcribe.exe`, run these commands. Replace the ID and choose an output folder you can write to.

```powershell
.\transcribe.exe recover-live list
.\transcribe.exe recover-live publish INTERVAL --output-dir '.\recovered'
```

Recovery publishes available committed words and reports missing coverage. To remove an unwanted recovery entry, run `recover-live discard INTERVAL --confirm`.

## Transcribe completed files

### Transcribe an MP4 offline

Each local job transcribes the first embedded audio track of one completed MP4. Windows Media Foundation decodes audio. Candle runs Whisper `base.en` on the CPU. Install the English model once. The worker verifies the pack before inference.

1. Download and extract the [Whisper base.en model ZIP](https://github.com/scubamount/AudioCapture/releases/download/v2.2.0/AudioCapture-Whisper-base.en-model.zip).
2. Open **Transcribe files…** in the recorder.
3. Set **Model pack folder** to the folder containing `config.json`, `tokenizer.json`, and `model.safetensors`.
4. Select **Import base.en pack**.
5. Choose your **MP4 file**.
6. Choose an output folder under **Save transcripts in**.
7. Select **Preview local job**.
8. Check the source, model, and destination.
9. Select **Start local transcript**.
10. Read the segments under **Timed transcript** when the job finishes.

Use **Cancel job** to stop at a processing checkpoint. Use **Current job** to return to the job panel. The output folder contains one job directory with `transcript.txt` and `transcript.json`. [Model setup](https://github.com/scubamount/AudioCapture/blob/v2.2.0/docs/images/local-mp4.png) · [Timed review](https://github.com/scubamount/AudioCapture/blob/v2.2.0/docs/images/local-mp4-review.png)

Audio stays on this computer. The worker decodes 16 kHz mono audio in windows of up to 30 seconds. It carries unfinished audio into the next window when Whisper supplies a suitable segment boundary. Segment times start at the first decoded sample. A delayed audio track can have a different clock in a video player.

The English model pack is a separate download. Verify the ZIP against its named row in the release checksum file. An existing `base.en` endpoint profile uses its configured service. [Compatible model revision](https://huggingface.co/openai/whisper-base.en/tree/911407f4214e0e1d82085af863093ec0b66f9cd6)

### Control a local MP4 job

Import the model from the recorder first. Create a grant for each agent from a trusted operator session. Give the grant access only to the input and output locations the agent needs. This example uses an existing file and output directory.

```json
{
  "input_roots": ["C:\\Media\\Meeting.mp4"],
  "output_roots": ["C:\\Transcripts"],
  "operations": ["batch.transcribe", "batch.status", "transcript.fetch"],
  "profiles": ["local-whisper"],
  "allow_headless_approval": true
}
```

Save the policy as `agent-policy.json`. Use the actual spelling of the selected paths. Run the commands in PowerShell 7 from the extracted app folder. Provision the grant in an operator shell without an existing `AUDIO_CAPTURE_GRANT_REF`.

```powershell
$grant = .\transcribe.exe control grants provision --json --policy .\agent-policy.json | ConvertFrom-Json
if (!$grant.ok) { throw $grant.error.message }
$env:AUDIO_CAPTURE_GRANT_REF = $grant.grant_ref
$request = @{
    schema_version = 1
    operation = 'transcribe'
    files = @('C:\Media\Meeting.mp4')
    profile_id = 'local-whisper'
    output_dir = 'C:\Transcripts'
}
$plan = $request | ConvertTo-Json -Compress | .\transcribe.exe control batch preview --json --input - | ConvertFrom-Json
if (!$plan.ok) { throw $plan.error.message }
$plan.scope
```

Review the scope. Run the next commands to approve and check the job. Reuse the same key when retrying a commit.

```powershell
$approval = @{schema_version=1; plan_id=$plan.plan_id; approved=$true; idempotency_key=[guid]::NewGuid().ToString()}
$job = $approval | ConvertTo-Json -Compress | .\transcribe.exe control batch commit --json --input - | ConvertFrom-Json
if (!$job.ok) { throw $job.error.message }
$query = @{schema_version=1; batch_id=$job.batch_id} | ConvertTo-Json -Compress
$query | .\transcribe.exe control batch status --json --input -
```

Run `transcript fetch` with the same query after completion to read results. To enable cancellation, include `batch.cancel` in the policy. Use `batch cancel` with the job ID.

To revoke a grant, run `control grants revoke --json --grant GRANT` from the operator shell. Replace `GRANT` with the issued reference. Keep the reference private.

Use `audio-capture.exe control` for a windowless CLI launch from another application. It accepts the same arguments as `transcribe.exe control`.

For MCP clients, launch `audio-capture.exe mcp`. It passes redirected input and output to the worker. Supply your executable path and grant reference.

```json
{
  "mcpServers": {
    "audio-capture": {
      "command": "C:\\Apps\\AudioCapture\\audio-capture.exe",
      "args": ["mcp"],
      "env": {"AUDIO_CAPTURE_GRANT_REF": "REPLACE_WITH_GRANT"}
    }
  }
}
```

Use `tools/list` for available operations. Call `audio_capture_batch` with `action` set to `preview`, `commit`, or `status`. Use `audio_capture_transcript` with `action` set to `fetch` to read timed text.

### Use an endpoint for completed files

The endpoint workflow uses the endpoint, model, API key variable, output folder, and timestamp setting shown in **Transcription settings**. You can load a config or enter the settings once. The service must accept multipart requests at `<base-url>/audio/transcriptions`.

1. Open **Transcribe files…** and select **Use an endpoint…**.
2. Select the saved audio or video files.
3. If the panel asks for the service URL, enter it in **Endpoint reply**.
4. Choose a shared timeline for synchronized tracks from the same recording.
5. Choose from the available options for oversized audio files and local HTML review.
6. Check the listed inputs, endpoint, model, and output folder.
7. Enter `y` in **Endpoint reply** at the final send prompt.
8. Select **Send reply**.

Answer each question with **Send reply**. Use **Close session** to close the endpoint panel. The worker reports each result. Live work takes priority when it shares the endpoint with a batch. A loopback URL uses a service running on your computer.

To reuse your settings, open **Transcription settings** and select **Load config…** before opening a new batch. For automatic loading, place your active `transcription.json` beside the executables. The example in `config/` is a template.

The endpoint picker can open a finalized recording timeline file such as `Recording-<id>.timeline.jsonl`. Choose the separate tracks or the mix to avoid duplicate speech.

You can also start the guided console from PowerShell. It reads `transcription.json` beside `transcribe.exe`. An empty output folder in this standalone mode uses `transcripts` beside the first input.

```powershell
.\transcribe.exe --guided -- '.\Discord.opus' '.\Microphone.opus'
```

Select **Endpoint queue…** to manage these batches. These PowerShell commands also work. Use the ID from the list.

```powershell
.\transcribe.exe batch-queue list
.\transcribe.exe batch-queue status ID
.\transcribe.exe batch-queue pause ID
.\transcribe.exe batch-queue resume ID
.\transcribe.exe batch-queue cancel ID
```

Resume returns eligible work to the queue. An unknown result might have completed at the endpoint. Check the service before you run `batch-queue resume ID --retry-unknown`.

### Transcribe a video

For local recognition, follow [Transcribe an MP4 offline](#transcribe-an-mp4-offline). For an endpoint, use the endpoint file picker. You can include audio files in the same endpoint batch.

You can also use the guided command with your saved transcription settings.

```powershell
.\transcribe.exe --guided -- '.\meeting.mp4'
```

The worker extracts the first embedded audio track locally. It sends only 16 kHz mono WAV audio to your endpoint. A long track is split automatically at the upload limit. The original video stays on disk.

The picker accepts MP4, M4V, MOV, MKV, WebM, MPEG, MPG, AVI, and WMV. Decoding depends on the codecs installed in Windows. A video with no readable audio produces a file error. Import uses Windows Media Foundation and needs no FFmpeg installation.

Transcript timestamps start at the first decoded audio sample. For edited videos or delayed audio tracks, timestamps can differ from the video player's clock.
HTML review plays the original file through your browser. Playback depends on its codec support and choice of audio track.

### Set a bearer token for the session

If you need a token, open PowerShell in the extracted folder. Enter the token in the masked prompt. Start the recorder from that shell so its companion inherits the variable.

```powershell
$secret = Read-Host 'Transcription API key' -AsSecureString
$env:TRANSCRIPTION_API_KEY = [Net.NetworkCredential]::new('', $secret).Password
.\audio-capture.exe
```

Enter `TRANSCRIPTION_API_KEY` in the live settings or when the endpoint workflow asks for a variable name. Run scripted commands that use this variable from the same shell. The worker reads its value from the environment.

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
| `--provider-profile openai-diarized-v1` | Requests speaker-labeled segments from an endpoint and model that support this profile. Use it without `--timestamps`. |
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

For oversized audio files, put `ffmpeg.exe` on PATH and enable the guided split option or `--chunk-long`. Video imports use the native splitter automatically. The worker sends one bounded 16 kHz mono WAV part at a time.

Put `ffprobe.exe` on PATH to validate full-file segment times for HTML review. The text remains available when the service returns no usable timing or ffprobe cannot check the duration. Restart Audio Capture after an environment change so the recorder and its companion see the updated PATH.

### Review and resume

Open the generated HTML in your browser. Search the text, play or loop timed segments from their source files, and export corrections as text or JSON. Keep the original recordings at their current paths for playback. The review runs locally.

Use a shared timeline only for synchronized tracks from one recording. For unrelated recordings, leave the guided choice at no or omit `--same-session`. Each file then has its own timeline. When you record in **Both** mode, choose either the mixed file or its separate tracks to avoid duplicate speech.

To resume a batch, use the same output directory, source paths, audio, endpoint, model, and request options. Keep `.audio-capture-jobs` with the results. Choose a fresh HTML path when you request another review. Matching completed jobs reuse local results even when the endpoint is offline. An uncached job can contact the endpoint.

For a fully cached run, leave the key prompt blank or omit `--api-key-env`. If you specify a key variable, it must be set even if all jobs are cached.

A job with an unknown result may have completed on the server. Check the service before you use `--retry-unknown`. Keep the same output directory to retain completed work.

## Review speakers and export memory

Use completed audio and its transcript. Detected speakers receive labels within the session. Inferred names cite transcript evidence. A review can confirm a person or correct the text.

For finalized live text, import its segment index with `refine-live`. Use the resulting session in `review-session`. To refine from saved audio, use the same session with `--timeline`, `--audio`, and `--adapter-config`. Run `refine-live --help` for the full command.

Live speaker labels and recognition speech gating are evaluation features. Use the completed-file workflow for speaker review with a local adapter.

For Discord and microphone recordings, use **Separate files** when possible. A test used the Pyannote and TitaNet models on 24 synthetic calls with reference text and times. Separate tracks had 7.66% diarization error and recovered 91.82% of isolated turns. Mixing those calls gave 21.38% error and 79.09% recall, missing both test targets.

1. Run `.\transcribe.exe prepare-session --session .\session.json` to choose inputs and create a session manifest.
2. Run `.\transcribe.exe attribute --session .\session.json` to process those inputs.
3. Run `.\transcribe.exe review-session --session .\session.json` to check speakers, text, and evidence.

Repeat `--session` in the attribution command to process more sessions. Use `--json` with `review-session` to get the latest revision. Use `--apply .\edits.json` to submit a correction file.

Diarization can use speaker data from a supported recognition result. For a result without speaker labels, an optional local adapter can identify speech intervals. The worker retains untimed text and leaves uncertain speaker assignments open for review.

The portable ZIP contains `tools/diarize_local.py`. Install its Python runtime, models, and FFmpeg separately. Use an adapter only in a manifest you trust. [Speaker setup and examples](https://github.com/scubamount/AudioCapture/blob/v2.2.0/docs/speaker-workflow.md)

Optional automatic naming uses English introductions and supported question-and-answer exchanges. Separate tracks or provider-labeled turns can support an inferred name. For a single mixed track grouped only by the local adapter, names stay as suggestions until you review the speaker assignments or confirm an identity.

Create `scope.json` with the manifest path and revision you approve. This example selects all turns in revision 2 of one session.

```json
{"schema_major":1,"sessions":[{"manifest":"session.json","revision":2}],"include_provisional_names":false}
```

Run `.\transcribe.exe export-memory --scope .\scope.json --output .\memory` to preview it. Add `--approve` to write a local JSONL batch. The export contains approved text and speaker data. Audio and credentials stay local.

Copy the batch manifest path from the export receipt. Run `.\transcribe.exe import-memory --batch PATH --output .\agent-memory` to apply it locally.

## Troubleshooting

- **Discord or microphone is quiet.** Check the selected process and input device. Refresh after a restart or device change. Check Windows microphone access and the two source meters.
- **A source drops out.** Check the recorder status. Other healthy sources can continue. Opus writes an Ogg checkpoint about once per second, which can leave a playable prefix after an interruption.
- **Live transcription is waiting.** A batch request may still own the available slot. Follow the status before relying on transcript coverage.
- **The live transcript could not save.** Use the displayed partial text path or the recovery command. Select a folder you can write to.
- **The transcription worker fails to start.** Check that `transcribe.exe` is beside the recorder.
- **Local model import fails.** Select the extracted release model folder. Confirm that it has the three model files and that the ZIP checksum matches.
- **The local video has no readable audio.** Check that the MP4 has a first audio track Windows can decode. Windows N editions can require the Media Feature Pack.
- **The worker cannot connect (Windows error 12029).** Start your transcription service. Check its host and port. Check the base URL prefix in the batch preview before you send.
- **The endpoint rejects the request.** Check the URL prefix, model name, token variable, media format, and upload limit. Use `--language` only with a code your service accepts.
- **A timed segment is missing.** Check that the endpoint supports `verbose_json` with segments and that ffprobe is on PATH for full-file review.
- **The review cannot play audio.** Check that the browser supports the format and the original audio remains at its recorded path.

## Terms and credit

This project is provided as-is for educational and personal use.

The Rust recorder follows the original AudioCapture project by masonasons. It uses the Win32 interface, WASAPI and Media Foundation, libopus, Ogg, and FLAC. [Original AudioCapture project](https://github.com/masonasons/AudioCapture)
