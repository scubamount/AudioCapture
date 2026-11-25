# Spike tasks

## 1. Establish the baseline

- [x] 1.1 Record the existing local model settings and the 24/32 kbps synthetic baseline results.
- [ ] 1.2 Prepare consented reference tracks and held-out examples with corrections and audio hashes.
- [ ] 1.3 Define text normalization and measures for word errors, technical terms, silence, timing, latency, memory, and cost.
- [ ] 1.4 Record the exact installed local model and library versions before the controlled comparison.

## 2. Evaluate endpoint compatibility

- [x] 2.1 Send two local synthetic Opus files through streamed multipart requests to a prefixed loopback endpoint and verify the returned text files.
- [ ] 2.2 Test full endpoint URLs, absent credentials, redirects, text and JSON responses, unsupported fields, and capability fallback.
- [ ] 2.3 Test batch state, cancellation, transient and ambiguous failures, restart, output collisions, and bounded resources.
- [ ] 2.4 Test chunk boundary offsets, overlapping speech, uncertain track grouping, and paused or shortened timelines.
- [ ] 2.5 Test endpoint-specific media and size limits, including conversion to a temporary WAV.
- [ ] 2.6 Record a versioned matrix for Speaches and LocalAI. Check whisper.cpp through a compatible adapter before claiming compatibility.

## 3. Compare transcription quality

- [ ] 3.1 Run the local baseline on all reference tracks and save metrics and model configuration.
- [ ] 3.2 Run a stronger local model when available and an explicitly selected compatible endpoint when available.
- [ ] 3.3 Compare 24 and 32 kbps mono speech, noisy speech, quiet speech, technical terms, and silence.
- [ ] 3.4 Evaluate optional AI correction against raw text. Check numbers, names, additions, and omissions.
- [ ] 3.5 Measure timing, peak memory, requests, costs, and errors. Record unsupported formats or unavailable models.
- [x] 3.6 Record the preliminary quiet-speech normalization and silence pilot with its one-fixture limitation.
- [ ] 3.7 Evaluate normalization against the held-out quiet and noisy tracks before choosing any default.
- [ ] 3.8 Compare faster-whisper VAD padding, silence thresholds, and batch sizes on short turns, quiet speech, timing, memory, and word error.
- [ ] 3.9 Evaluate optional WhisperX alignment on the same held-out inputs. Retain original segments and report untimed words and overlap errors.

The preliminary local VAD comparison used eight generated turns and a ten-second silence control. Both tested VAD settings produced 11 raw token errors. Disabling VAD produced 10 and inserted a word on silence. The token count includes number-format differences. This small synthetic comparison does not meet the held-out gate in the design.

## 4. Decide and hand off

- [ ] 4.1 Report held-out results against the proposed quality gates and explain any gaps.
- [ ] 4.2 Select a model and workflow only if evidence supports them. Otherwise propose the next bounded test.
- [ ] 4.3 Draft a separate implementation change for the chosen workflow and provider capabilities.
