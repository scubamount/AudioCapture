# Design

## Context and evidence

The recorder writes Opus, WAV, MP3, and FLAC. Separate and combined modes share a session timeline. Pause and silence skipping remove intervals from that timeline. Existing filenames contain a session identifier and track name, but a durable session manifest does not exist.

The previous local experiment used faster-whisper `base.en`, CPU int8, English, beam size 5, and voice activity detection. Two synthetic sentences produced the same words from 24 kbps and 32 kbps mono Opus. The files contained 21,068 and 28,020 bytes. A temporary Python and HTTPX client sent both as streamed multipart requests to a loopback faster-whisper server at `/custom/gateway/v1/audio/transcriptions`. The requests used an explicit model alias and a named key environment variable. Two text outputs matched both reference sentences.

A separate synthetic pilot lowered the same 15-word passage to 4% amplitude. The local baseline produced one insertion. Peak normalization to 0.8 produced zero errors. Quiet speech with seeded Gaussian noise at RMS 0.002 and digital silence produced no errors or false words with or without normalization. These small trials do not establish accuracy for live conversations, crosstalk, other languages, or remote providers. Normalization remains an experiment.

## Spike questions and interface

The spike will determine whether a stronger local or endpoint model improves word accuracy and technical terms enough to justify latency, cost, and resource use. It will also test whether optional text correction preserves meaning and improves readability. One explicit command selects one or more completed files or sessions, a target model, and the output location. A future UI may expose the same choices.

The separately supplied experimental script exposes this interface. Start a compatible local endpoint before running it. `ASR_KEY` is the name of an environment variable, not a key value. The script is a protocol probe and does not integrate with the recorder.

```powershell
python .\transcribe_probe.py --base-url http://127.0.0.1:8000/v1 --model example-asr --output-dir .\transcripts --api-key-env ASR_KEY .\Discord.opus .\Microphone.opus
```

The future workflow can select files, a session, or a manifest. Provide language, terminology hints, and authentication only when selected. Configuration identifies the target host and model without containing the key. Use an environment variable or Windows Credential Manager for a key. An empty key supports local services. Display the endpoint host, selected sources, output location, and known cost information before execution.

## Endpoint contract

The minimum compatibility target is a multipart `POST` to the selected OpenAI-style audio transcription endpoint with a `file` part and `model` field. A base URL ending in `/v1` appends `/audio/transcriptions` once. Preserve any configured path prefix. Alternatively accept an explicitly supplied full transcription URL. Reject query strings or credentials in the configured URL and never place secrets in a command example or log. Test base URLs with and without trailing slashes and with reverse-proxy prefixes.

Accept plain text and JSON `text` responses. Treat segment and word timestamps, speaker diarization, language hints, vocabulary prompts, and response formats as endpoint/model capabilities. Probe only on explicit command or use configured capabilities. Do not silently claim a capability based on an endpoint label. Unsupported optional fields can be removed after an explicit unsupported-field response, with a warning and recorded effective settings. A response without timestamps remains untimed. Invalid results and unsupported formats are reported for that item.

OpenAI's speech-to-text guide documents its own current formats and model capabilities. Other compatible services can differ. Query or configure each service's accepted media and size limits. When a service rejects Ogg Opus, decode a temporary compatible WAV with a recorded sample rate and channel count. Enforce endpoint-specific limits on the converted file, and preserve the original. [OpenAI speech-to-text guide](https://developers.openai.com/api/docs/guides/speech-to-text)

Allow HTTPS endpoints and HTTP loopback or trusted LAN endpoints chosen by the user. Identify HTTP clearly in the selected endpoint details. Never forward audio or authorization to another host through a redirect. Redact authorization and private paths from diagnostic output. Do not send recording audio to any service during discovery, recording, listing, opening, or app startup.

## Tracks, timing, and jobs

The user selects completed files or a confirmed session mapping. Infer related tracks only when the existing recorder filename identity is unambiguous. Present ambiguous mappings for explicit selection. Do not treat a combined file as an additional source when its constituent tracks are already selected. Do not infer distinct people from application tracks. Preserve source labels, including microphone and the process name.

Transcribe separate tracks independently. Merge segments by reported recording-time offsets, retaining concurrent segments and source labels. Reconstruct split WAV parts in order only with confirmed identity and offsets. A combined-only track retains an unknown speaker label unless the provider supplies reliable diarization. Do not convert output time to wall time when pauses or silence were removed and no mapping is available. Store that timeline limitation with the transcript. Preserve the raw per-source result and produce a readable merged transcript when timing permits.

Each batch job has immutable source references, content identity, endpoint/model settings, and a persisted state. The client queues ordinary transcription requests. It does not require a provider batch API. Use bounded concurrency and streaming file reads. Make file and endpoint size limits configurable. For large recordings, evaluate decodable chunks with known offsets and bounded overlap. Convert each returned chunk-relative timestamp by adding the chunk's recorded source offset, then add any confirmed part offset. Deduplicate overlap only when timestamps or matching text support it. Text-only responses remain untimed. For an oversized indivisible input, report the failing item and retain other results. Use per-job temporary files with a size bound and clean them after success or cancellation. Preserve original audio and successful transcripts.

Track queued, running, complete, failed, cancelled, and unknown states independently. Honor provider retry guidance and retry bounded transient failures with backoff. A timeout after upload may have caused provider processing or charges. Mark it unknown and require an explicit retry rather than silently duplicating the request. Resume known incomplete jobs without overwriting completed outputs. Cancellation stops pending work and signals in-flight requests where possible. It cannot guarantee remote cancellation after upload.

## Evaluation and decision gates

Prepare consented, manually corrected reference audio with app and microphone tracks. Target at least 30 minutes of speech from four or more voices, plus five minutes of labeled silence. Include quiet speech, overlapping turns, technical names, and 24/32 kbps variants. Reserve at least 10 minutes of speech and two minutes of silence as a held-out set. Keep speakers and source recordings disjoint between selection and held-out sets where possible. Mark the gate untested if the held-out set is smaller or unrepresentative. Record model version, audio hash, decoding settings, and normalization rules.

Measure word error rate as substitutions plus deletions plus insertions divided by reference speech words. Report aggregate and per-source results. Measure false speech separately as the proportion of labeled silent duration containing emitted words, and report the number of emitted words. Do not divide by the zero reference words in silence. Measure technical-term accuracy, timestamp errors when available, real-time factor, peak memory, endpoint cost, and output size. Capture failures and missing capabilities as well as successful results. Compare against the local `base.en` baseline with identical inputs. Evaluate local faster-whisper `large-v3` when resources permit. Also evaluate a deployment alias serving `whisper-large-v3` or a GPT transcription model when a configured endpoint confirms support. Record the actual model identity and capabilities. Test at least one compatible endpoint when available.

Candidate acceptance for the held-out set requires at least a 10% relative word-error reduction versus baseline, no more than a 1 percentage point increase in false speech rate, and no regression in technical-term accuracy. For timestamped output, report median and 95th percentile absolute segment boundary error. Report actual latency, memory, and cost per audio hour rather than assuming a budget. The user's quality and cost priorities may change the candidate threshold. If no candidate meets the gate, retain the baseline or propose another experiment.

If AI correction is evaluated, preserve the raw text and compare corrected text against the same reference. Flag additions, omissions, changed numbers, names, and uncertain terms. Use a separate opt-in correction step, record its provider/model, and reject a correction that invents words or increases error on the held-out set. Treat the uncorrected transcript as authoritative evidence.

## Risks and unresolved decisions

Endpoint compatibility varies by model, response format, size limit, authentication, and timestamp support. The spike must record actual observations and unsupported cases. Local inference requires available model files and compute. External evaluation incurs charges and transmits selected content. Provider selection, default model, implementation language, UI integration, and any automatic cleanup remain decisions for a later proposal.
