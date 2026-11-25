# Live speech qualification

The released whole-file diarizer and the live candidate used the same models and scoring rules. Each layout received the same audio in both runs.

## Speaker labels

The second live candidate passed the separate-track targets on the 24 released synthetic calls. It missed the accuracy and delay targets on four new calls.

| Corpus and layout | Adapter | Diarization error | Isolated turns recovered | Overlap error |
| --- | --- | ---: | ---: | ---: |
| Four new calls, separate tracks | Released, whole file | 8.00% | 10/11 | 9.72% |
| Four new calls, separate tracks | Live, 24-second history | 29.86% | 8/11 | 19.44% |
| Four new calls, mixed | Released, whole file | 32.14% | 8/11 | 40.28% |
| Four new calls, mixed | Live, 24-second history | 45.42% | 6/11 | 34.72% |

The table pools scored speaker time across four 40-second calls. The calls contain 24 annotated turns. The scorer excludes 125 ms on each side of reference boundaries. [Comparison chart](README.md#live-speaker-labels)

On separate tracks, 16 turns received a correct label over half their duration. Eight remained unresolved. Five assigned turns took more than 15 seconds. Assigned-turn p95 was 22.53 seconds in the first trial.

Scored independently, the live program track reached 37.16% error and recovered 7 of 10 isolated turns. The released adapter reached 11.28% and recovered 9. The live microphone reached 10.87% error against a released baseline of zero. Both recovered all five isolated microphone turns. Independent scoring ignores simultaneous speech on the other track.

Delay uses the end of a reference turn and the earliest correct assignment that survived later revisions. It combines paced source availability with measured inference times. It excludes model startup, recognition, and UI publication.

The first live candidate used a 12-second history and committed each core immediately. Its separate-track error reached 28.29% on the released 24 calls. The whole-file baseline reached 7.66%. The second candidate revises the previous 24 seconds.

On those 24 calls, the second candidate reached 7.66% error, recovered 202 of 220 isolated turns, and had zero scored overlap error. In the first trial, 248 of 266 turns received a stable correct assignment. Eighteen remained unresolved. Assigned-turn delay p95 was 8.91 seconds.

The first candidate processed two hours of two-source audio in 1,309 seconds. Private memory rose from 668.96 MiB at 30 minutes of input to 669.87 MiB at two hours. Peak sampled working set was 192.53 MiB. This accelerated run processed both sources through the same warm model instance.

The second candidate's paced run measured 669.09 MiB at 600 wall seconds and 668.95 MiB at 1,800 wall seconds. Its process tree was sampled every 30 ms. The 30-minute sample occurred at 03:39:03 UTC on November 25. A separate accelerated run logged 6,400 source seconds. Testing stopped before either run reached two hours. Two-hour wall-clock memory growth for this candidate is **not checked**.

The following rows show the first paired trial. Warm processing time is wall seconds per second of call audio. Peak memory includes the adapter and its children.

| Corpus and layout | Whole-file peak MiB | Live peak MiB | Live warm processing | Median cold start, seconds |
| --- | ---: | ---: | ---: | ---: |
| Released 24, separate tracks | 237.59 | 255.89 | 0.409 | 0.49 |
| Released 24, mixed | 253.47 | 345.92 | 0.508 | 0.97 |
| Four new calls, separate tracks | 191.13 | 195.46 | 0.301 | 0.45 |
| Four new calls, mixed | 181.89 | 198.38 | 0.173 | 0.42 |

On the mixed released corpus, the whole-file and live adapters both reached 21.38% error and recovered 174 of 220 isolated turns. On the four mixed confirmation calls, 12 of 24 turns remained unresolved.

Five alternating paired trials reproduced the same held-out error and recall. Durations below give per-trial p95 among correctly assigned turns. The timing includes competing workloads on the host.

The released 24-call corpus completed five paired trials on separate tracks and four on the mix. The first-trial values above use the same input hash in each pair.

| Live layout | Warm seconds per call second, trials 1–5 | Assigned-turn p95 seconds, trials 1–5 | Median p95 |
| --- | --- | --- | ---: |
| Separate | 0.301, 0.540, 0.405, 0.263, 0.265 | 22.53, 22.86, 22.94, 22.44, 22.06 | 22.53 |
| Mixed | 0.173, 0.304, 0.301, 0.161, 0.178 | 27.64, 28.36, 29.20, 27.65, 27.80 | 27.80 |

Six development calls added a 32-second silent pause. The whole-file baseline reached 4.07% error and 96.36% isolated recall. Allowing corrections across a longer gap improved a prototype to 6.99% error and 94.55% recall. Its p95 delay reached 47.79 seconds. A direct-embedding prototype reached a 10.76-second p95, with 85.45% recall and 342.41 MiB peak memory. Both missed a development target. The unused four-call speaker confirmation set stayed sealed.

These tests use three synthetic Windows voices. Accuracy on annotated human calls is **not checked**. Speaker results do not qualify a naming model.

Live labels are anonymous and apply only to the selected source during the current speaker interval. The accuracy test scores those labels. It contains no cross-session identity or inferred-name test.

## Recognition gating

The first candidate trimmed quiet runs inside recognition chunks. It reduced upload duration by 85.84% on four sparse calls. It also lost one annotated quiet word that the ungated run recovered. Its improved word error rate did not satisfy the boundary requirement.

| Measure per four-call set | Ungated | Trimmed chunks |
| --- | ---: | ---: |
| Submitted source seconds | 320.00 | 45.32 |
| Complete multipart body bytes | 30,753,264 | 4,360,265 |
| Recognition requests | 80 | 23 |
| Final UTF-8 TXT bytes | 2,693 | 2,426 |
| Substitutions / deletions / insertions | 8 / 5 / 1 | 4 / 2 / 0 |
| Word error rate, 135 reference words | 10.37% | 4.44% |
| Quiet first and last words recovered | 14/16 | 15/16 |

The candidate gained two boundary words and lost one. Counting total matches alone conceals that loss. The dense control submitted all 5.16 seconds in both modes. Each transcript had four deletions among 21 reference words.

Five alternating pairs produced the same counts with frozen responses. One additional pair used a warmed endpoint and real-time input. It reproduced the same word errors, requests, and bytes without gaps. Quiet onset matches rose from seven to eight. Each configuration recovered seven quiet tails, on different utterances.

The source tracks contained 36.298 seconds of annotated speech during 320 source seconds. The test replayed each track as 48 kHz mono PCM16. The worker sent WAV files to a local OpenAI-compatible multipart endpoint.

The endpoint used faster-whisper 1.2.1 with base.en, CPU int8, two threads, beam size five, English, and endpoint VAD. The model SHA-256 was `2a166925539a16005f14ff328359f9b9adb9dc4fb631bb3b227526862e93e2ef`. The worker SHA-256 was `bdc64cbd173307e7736d4d4102b6351a192ab5f2003197a3d6d5e1afdc07ba0b`.

The decoder used independent requests, no previous text, and no prompt. Its default temperature fallback used 0.0 through 1.0 in steps of 0.2. It used five candidates when sampling. No random seed was set. The default compression-ratio, log-probability, and no-speech thresholds were 2.4, -1.0, and 0.6.

An initial concurrent, cold-endpoint run exceeded the 15-second request limit and published gaps. For the controlled accuracy comparison, the worker first sent its exact WAV payloads to a local collector. Whisper recognized each of the 60 distinct WAV inputs. The worker repeated the trials with responses indexed by WAV SHA-256. These trials measure recognition quality and upload size. They exclude recognition latency. [Endpoint and scoring harness](../../rust/tools/live_quality_asr.py)

### Guarded whole chunks

The next candidate kept each source's original four-second chunk boundaries. It can omit an entire chunk only when every PCM16 sample is zero. A nonzero sample within the preceding 240 ms or following 160 ms also protects the entire chunk. The gate keeps partial chunks.

On the original four calls, it sent 136 of 320 source seconds in 34 requests. The ungated worker sent 80 requests. The same warmed endpoint received byte-identical retained WAV files at identical source positions. The two runs produced the same 2,693 TXT bytes, word errors, and recovered quiet words. The dense control retained both requests. The worker SHA-256 was `f1a10ba85b10a51ac915ebd2221ecaec381299d43eec9cf95df173c94e55d04c`.

A separate recorder-core replay injected 960,017 frames per source. Recording alone, ungated recognition, and guarded recognition produced identical complete program and microphone WAV files. Their respective request counts were zero, 12, and four.

**The guarded candidate remains evaluation-only.** The independent set used four new 48-second scripts and four different Flite voices. It contained 42.05 annotated speech seconds across 384 source seconds. Two of five planned alternating trials completed. Trial three completed two paired calls and one further ungated call before testing stopped.

| Independent four-call set | Ungated, trial 1 | Guarded, trial 1 | Ungated, trial 2 | Guarded, trial 2 |
| --- | ---: | ---: | ---: | ---: |
| Submitted seconds | 384 | 192 | 384 | 192 |
| Requests | 96 | 48 | 96 | 48 |
| Multipart body bytes | 36,903,968 | 18,451,920 | 36,903,968 | 18,451,920 |
| Final TXT bytes | 2,986 | 3,020 | 2,986 | 3,004 |
| Substitutions / deletions / insertions | 11 / 1 / 1 | 11 / 1 / 8 | 11 / 1 / 1 | 11 / 1 / 2 |
| Word error rate, 144 reference words | 9.03% | 13.89% | 9.03% | 9.72% |
| Quiet onsets / tails recovered, eight each | 5 / 6 | 5 / 6 | 5 / 6 | 5 / 6 |

All 12 completed case and control pairs preserved every retained WAV's bytes and source range. They lost no previously recovered boundary word. In trial one, the endpoint produced different text for an identical retained WAV. No omitted zero core produced text in the ungated run. The first trial exceeded the one-percentage-point word error budget. The dense control sent the same 6.907 seconds in two requests in both trials. Both modes recovered 15 of its 22 words.

The third trial's incomplete results are excluded from the table. All runs used the same warmed local endpoint settings and real-time source pacing. The worker's complete receipts contain 25 case and mode results. The SHA-256 of that partial aggregate was `00f020387b3d020f9fc9efff1815660ebd488a1a4cf0af9bea522fbdd908862b`.

## Recording storage

Each row sums two Ogg Opus containers. Both encoders used 32 kbps mono. The replay used decoded input samples and the recorder's shared 10 ms silence rule.

| Input | Remove recording silence | Saved frames per track | Regular Opus bytes | Speech DTX bytes |
| --- | --- | ---: | ---: | ---: |
| Saved Discord and microphone | Off | 1,396,203 | 157,593 | 69,173 |
| Saved Discord and microphone | On | 1,396,203 | 157,593 | 69,173 |
| Synthetic sparse call | Off | 1,920,000 | 136,198 | 54,879 |
| Synthetic sparse call | On | 408,960 | 48,148 | 40,073 |

The saved call had no shared block below the silence threshold. The sparse call retained 8.52 seconds per track from 40 seconds. Speech DTX preserved duration when recording silence removal was off.

## Reproduction

The reference host ran Windows 11 Pro x64 build 26200 on an AMD Ryzen 7 7800X3D. It had 31.61 GiB usable RAM, a Samsung 970 EVO NVMe drive, and the Balanced power plan. Other benchmark and build processes shared the host.

The speaker backend used sherpa-onnx 2.0.3, two CPU threads, clustering threshold 0.5, and 16 kHz mono PCM. Live requests contained four-second cores and a final short core where needed. Each call started a fresh model process.

| Input or implementation | SHA-256 |
| --- | --- |
| Released 24-call manifest | `ac0a84bf01887c0a5df24c97c078592a7993f58c325b5ed227fd4f5ac9d7ce54` |
| Initial four-call manifest | `2799099f9136d7ffad4017ed26c2d4c3f1fc524e029878bbcb05480a14a460d9` |
| Second four-call manifest | `93ac8eddc4211f966a18394041820850d9c5155ef3b8369c73732c8f5b2c1b1e` |
| Independent Flite speech-gate manifest | `ceb2ce5f77240c72a203dc35cece2598e77c18a54d7c1edad55b75c4ed625ec5` |
| Released Python adapter | `a8fa75e25b0ce0bd91ded999e22e754e64600abe5c5d0d2654f83d3a38362513` |
| Second live adapter | `b789475348dd8ed27fbe34964ef45cc2fdd4944447a2f71da1bec42e661c4067` |
| Segmentation ONNX model | `220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079` |
| TitaNet-small ONNX model | `ad4a1802485d8b34c722d2a9d04249662f2ece5d28a7a039063ca22f515a789e` |

Run these commands from the repository root. Set the adapter JSON to local Python and model paths. Use a new directory for each generated corpus.

```powershell
python -B rust/tools/test_live_quality.py generate --confirmation .\confirmation
python -B rust/tools/test_live_quality.py acoustic --corpus .\confirmation --adapter .\adapter.json --bridge .\rust\tools\diarize_local.py --layout stems --mode live --output .\live-stems.json
python -B rust/tools/test_live_quality.py acoustic --corpus .\confirmation --adapter .\adapter.json --bridge .\released\diarize_local.py --layout stems --mode offline --output .\released-stems.json
python -B rust/tools/test_live_quality.py soak --corpus .\confirmation --adapter .\adapter.json --bridge .\rust\tools\diarize_local.py --seconds 7200 --output .\soak.json
python -B rust/tools/test_live_quality.py generate .\speech-fixture
python -B rust/tools/live_quality_asr.py --worker .\transcribe.exe --model .\base.en --corpus .\speech-fixture --output .\gate-results --speed 5 --precompute
python -B rust/tools/test_live_quality.py generate --gate-confirmation .\flite-fixture
python -B rust/tools/live_quality_asr.py --worker .\guarded-candidate\transcribe.exe --model .\base.en --corpus .\flite-fixture --output .\guarded-results --warmup .\speech-fixture\held01\program.wav --verify-guarded-cores
```

Use `--layout mix` for the mixed comparison. The harness reports unassigned turns, switches, overlap error, runtime, and sampled process-tree working set. The soak replays audio through one continuous warm model process. [Speaker harness](../../rust/tools/test_live_quality.py) · [Qualification targets](../../openspec/changes/add-independent-live-transcription/acceptance.md)
