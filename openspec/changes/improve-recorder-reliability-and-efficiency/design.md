# Design

## Baseline and priorities

The baseline is `ce7089fdc63937e681f8d9bb4621f488118f24cf`. The four-second interruption test produced a zero-byte Opus file. A synthetic source delayed by 500 ms stopped its session. A final-frame impulse disappeared from a 16 kHz WAV.

P0 covers Opus recovery, late packets, and feedback prevention. P1 covers source meters, overlap handling, WAV tails, imported gain, partial device discovery, packet allocations, and package identity. P2 covers pause-history work and optional compact-output guidance. This ordering puts conversation retention before speed.

WASAPI discontinuity and timestamp-error flags are distinct from an ordinarily late packet. A flagged source can continue while a healthy source records. Use a reliable device timestamp to locate a known gap. When the timestamp is unreliable, retain the last defensible position and report that the extent of loss is unknown. Never present an estimated interval as measured. Count and expose repeated flags without growing an event queue. A disconnected source remains a separate failure mode.

## Opus recovery

Finish an Ogg page and flush the file buffer at least once per second of active input. The flush must report an error. It must not close or restart the logical Opus stream. The last encoded packet may remain pending for final granule handling.

For an abrupt recorder-process exit, the recoverable prefix may lag the latest accepted input by at most one second plus two 20 ms Opus frames and the encoder lookahead. This bound assumes a writable local file and successful checkpoint writes. The test measures the actual lookahead and accepted sample count. It uses a decoder that accepts a complete Ogg prefix without an end-of-stream page. It verifies a known signal before the bound and verifies that no corruption appears inside the decoded prefix.

Normal stop must retain the full requested sample count and valid end granule. An application flush does not prove power-loss durability. Power-loss and forced storage failure tests are separate follow-up work.

## Late packets and timeline

The mixer commits 10 ms blocks on a shared 48 kHz timeline. A packet that crosses the committed cursor loses only its committed prefix. Insert the remaining suffix at its original timestamp. A wholly late packet is dropped. Count the dropped frames per source and expose the source and affected interval in the status.

Fill missing frames with silence on that source's track. Continue healthy sources and preserve their original timeline. Do not silently reinterpret malformed frames or implausible future timestamps as late packets. Preserve their existing error behavior. A repeated or sustained late source must continue to report loss without growing queues without bound.

The 200 ms watermark is a starting policy. Do not change it without a test that measures late packets, latency, and bounded memory under load. Avoid replacing real speech with guessed samples.

If a capture source exits during a multi-source recording, identify it and keep the healthy sources active. Preserve source labels and the shared timeline. After the lost source's last known position, its missing samples are silent. Identify the lost interval as uncertain when its extent is unknown. If the last source exits, stop the session and report the error. Fatal output-writer errors stop recording and finalize what can be finalized. Do not keep retrying the failed block or create duplicate samples.

## Routing and source activity

The application can detect system plus process capture on the default render endpoint and selected parent-child process trees. In mixed or monitored mode, reject known overlap before capture and name the conflicting sources. Keep separate-only mode available for intentional overlap. If overlap cannot be determined, show its uncertainty and allow the user to resolve source selection.

System loopback feeding the same render endpoint through monitoring can form a feedback loop. Capture pins its default endpoint at start. Compare the actual captured endpoint identity with the actual monitor endpoint identity before starting. Block the route when the identities match. If either identity cannot be determined, report the uncertainty and prevent monitoring until the route can be established as safe.

Recheck actual endpoint identities if routing changes during capture. A default-device change alone does not mean the pinned capture endpoint changed. Warn and disable monitoring if the actual endpoints match or safe routing can no longer be established. Continue independent recording.

Show a peak meter and a recent-activity indicator for each selected source. Compute levels from source samples after gain and before mixing. Update the UI at a bounded rate, such as four times per second. A quiet indicator is informational and must not mark an active but silent source as failed. Accessible names must include the source identity and level.

Also report input level before gain, retained peak and clipping. When mono output is selected, compare stereo input activity with its downmixed level. Indicate substantial opposing-channel cancellation without altering the audio. Test equal, one-sided, opposite-polarity, quiet, and clipped synthetic inputs. Clear or age the held indicator by a defined rule so it remains useful during long recordings.

## Recording markers

Allow an explicit marker during a session. Store its label and position in an Audacity-compatible point-label sidecar for that session. Represent position on the recorded output timeline. Pauses and silence skipping remove the same intervals as the audio. A marker in a removed interval maps to the adjacent output boundary. Sanitize label delimiters, protect existing files, and report write errors. Creating a marker must not request transcription.

Do not hold the marker queue lock while rendering audio, invoking an encoder, or flushing a sidecar. Move pending requests out of the lock before processing. Queue behavior must remain bounded and report overflow or persistence failure.

## Encoder and timeline efficiency

Use a fixed-size temporary encoder buffer and retain only the produced Opus bytes for Ogg packet ownership. Preserve packet contents, ordering, pre-skip, granules, and final duration. Periodic page boundaries can change container bytes. Compare decoded audio and timing across the complete checkpoint-enabled file.

Consume pause transitions once and retain the current pause state. Keep transitions until every committed block that needs them has been rendered. Preserve the single shared pause and silence masks for all tracks. Bound queue sizes and expose overruns.

Separate-track mode already avoids a redundant mixed encoding pass. Keep all three file modes and the current application defaults. Add concise guidance that separate files retain source identity. Keep the existing 32 kbps mono workflow available and offer 24 kbps mono as an option. The review found a 24.8% reduction on one synthetic sample with two recognized phrases. It did not establish quality on noisy calls.

## Output and compatibility

For downsampled WAV, account for FIR group delay, flush the tail, and trim or pad to the intended output duration. Verify beginning and end impulses at 16 and 44.1 kHz. Verify alignment with 48 kHz output and chunk boundaries. Preserve rollover frame counts.

Preserve imported gain with 1% resolution or keep its exact value until edited. Opening, saving, starting, and closing without editing must not round it. Continue to read legacy and current settings.

Enumerate the process snapshot independently of active audio sessions. If one endpoint or session fails, retain available processes and identify the incomplete active-audio data. Filtering off must still list the process snapshot. Do not treat a missing session as proof that a process is silent.

Package only the executable and dependency notices bound to the same locked source build. The helper can perform the locked release build or verify a build receipt with source, lockfile, target, flags, and binary hashes. Detect a changed source or lockfile before output. Keep the portable build's static-runtime flags.

## Verification and measurement

Use generated signals and scratch files. Do not use private conversations for automated tests. Add targeted tests with names reflected in `tasks.md`. Run the affected Rust tests from `rust/`:

```powershell
cargo test --locked --lib opus_writer::tests
cargo test --locked --lib record::tests
cargo test --locked --lib encode::tests
cargo test --locked --lib prefs::tests
cargo test --locked --bin audio-capture
```

Run the existing isolated timeline benchmark with `cargo test --release --locked --lib record::tests::benchmark_timeline -- --ignored --exact --nocapture`. Add a reproducible allocator harness for ten seconds of 48 kHz stereo input encoded as 32 kbps mono. Report peak live Rust heap bytes, packet count, decoded duration, and output size. Compare the original packet allocation against the proposed allocation using the same checkpoint policy. Hold toolchain, release flags, fixture, and machine constant. The change passes only if peak live Rust heap bytes decrease and decoded PCM and timing remain equivalent. Report native codec memory and full-process CPU separately when measured.

The review harness observed 1,061,488 peak Rust heap bytes before and 58,302 after a scratch allocation change. It does not measure native allocations or whole-process memory. Do not present these numbers as a production benchmark. Measure the checkpoint cost independently before asserting overall gains.

A checkpoint-matched encoder harness measured peak live Rust heap allocation of 241,500 bytes before the packet change and 41,619 bytes after it. The synthetic outputs were byte-identical. A separate stub-capture mixer benchmark with 1,000 prior pause transitions measured two-source medians of 75.86 ms before and 14.23 ms after for 100 seconds of represented audio. Without pause history, the respective medians were 11.15 ms and 13.20 ms. The latter path also computes source levels. These isolated measurements exclude native codec memory and full application CPU.

An FFmpeg experiment found a 0.59% size increase with one-second pages versus five-second pages on a single short synthetic fixture. This is context for the checkpoint interval. It does not establish Rust writer overhead. Matroska packaging and process-tree exclusion remain experiments until their separate duration, isolation, quality, and recovery tests pass.

## Optional Opus speech profile

Offer an explicitly selected `Opus speech (mono, DTX)` profile with the existing selectable Opus bitrate choices. Use libopus VOIP application mode with discontinuous transmission enabled. Record an ordinary `.opus` file with the same shared timeline, timestamps, checkpoint behavior, and recovery guarantees as the existing Opus mode. Preserve the existing application default and regular Opus settings. Explain that the speech profile targets compact voice recordings and requires a quality check for the user's conditions.

The local A/B used 53.125 seconds and 2,550,013 input frames at 32 kbps mono. For digital-silence intervals, regular Opus used 119,252 bytes and VOIP with DTX used 62,030 bytes. With approximately -54 dBFS white noise, the corresponding files used 193,097 and 72,585 bytes. A local transcription of the DTX sample misheard “Kubernetes.” This is one synthetic fixture. Broader held-out real-call quality remains unverified. Test both presets with short and quiet speech, noise, mixed speech, music, synchronization, normal final duration, and interrupted recovery before describing expected results for other recordings.

Run a one-minute Windows capture with selected app and microphone, then decode both files and check relative timing. Test real source loss and the endpoint-change path when hardware is available. Record the output size and CPU with separate and both file modes. These live checks remain pending until performed.
