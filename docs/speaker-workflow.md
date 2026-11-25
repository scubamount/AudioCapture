# Speaker attribution and memory export

Use `transcribe.exe` after recording and recognition. Speaker attribution links voice activity to existing transcript text. Review names and corrections before you export selected text as local JSONL.

The speaker workflow uses its own CLI commands for session preparation, review, and export. The Whisper `base.en` pack supplies English text and segment timing in offline MP4 jobs. For speaker data, use a supported diarized response or install the optional speaker adapter. [Offline MP4 workflow](../rust/README.md#transcribe-an-mp4-offline)

## Prepare a session

Place your audio and transcript files in a working folder. A transcript can be a UTF-8 text file, a supported recognizer response, or a completed worker job artifact. Use timed segments or words if you want to attach text to speakers.

Run the preparation command from PowerShell in the extracted app folder. Choose the completed audio and its recognition output when prompted.

```powershell
.\transcribe.exe prepare-session --session .\session.json
```

Enter one audio path at a time. Enter a blank line to finish the list. You can also pass audio paths after `--` and then enter the artifact for each path.

Use a shared clock for tracks from the same recording. Keep unrelated recordings separate. When a recording has a mix and all its separate tracks, prefer the separate tracks. Select one representation when using a mix or partial coverage, or when system and program captures overlap.

The local pyannote and TitaNet models processed 24 synthetic conversations. The test used reference text and timings to measure speaker attribution.

| Audio representation | Diarization error | Isolated-turn recall |
| --- | ---: | ---: |
| Separate program and microphone tracks | 7.66% | 91.82% |
| One mixed track from the same calls | 21.38% | 79.09% |

Separate tracks met the test targets of at most 20% diarization error and at least 90% isolated-turn recall. The mix missed both targets. The results cover these voices, recordings, and model settings. [Measurements and naming results](performance.md)

You can also write the manifest directly. This example refers to two synchronized sources. The files and identifiers are examples.

```json
{
  "schema_major": 1,
  "session_id": "s_demo",
  "shared_clock": true,
  "representation": "stems",
  "complete_stems": true,
  "sources": [
    {
      "source_id": "src_program",
      "role": "program",
      "audio": "call.opus",
      "artifact": "call.json"
    },
    {
      "source_id": "src_mic",
      "role": "mic",
      "audio": "microphone.opus",
      "artifact": "microphone.json"
    }
  ]
}
```

Resolve relative media and recognition paths from the manifest directory. Use unique, durable source IDs. The worker records the audio hash and rejects different audio assigned to the same ID. Use a new source ID for changed media.

The worker uses the recorded duration to check speaker intervals. Install ffprobe to inspect the source when the manifest omits `duration_ms`. If you supply the duration, use the decoded timeline in milliseconds.

## Choose the speaker data

### Existing provider results

The worker can read speaker and timing fields from supported recognition output. For an endpoint and model that implement the OpenAI diarized response contract, request the profile at transcription time.

```powershell
.\transcribe.exe --base-url 'http://127.0.0.1:8000/v1' --model 'speaker-asr' --output-dir .\transcripts --provider-profile openai-diarized-v1 -- .\call.opus
```

Replace the example model with the deployment name your endpoint expects. Use HTTPS for a remote service. This profile requests `diarized_json` and sends `chunking_strategy=auto`. Omit `--timestamps` with this profile. [Provider API contract](https://developers.openai.com/api/reference/resources/audio/subresources/transcriptions/methods/create/)

Point the manifest's `artifact` at the saved response or completed job record in `.audio-capture-jobs`. For a split file, use its `-combined.json` result. The job record and combined result retain request provenance.

If a result lacks speaker labels, a local adapter can infer speaker activity from the original audio. The worker uses valid text timings to align the two results. With no usable timing, the worker keeps the full text untimed and leaves its speaker unknown. If an explicitly selected adapter fails, review retains the transcript and reports the attribution failure.

### Optional local adapter

The adapter reads the selected audio and returns speaker intervals. Install it separately from the recorder. The project includes the `tools/diarize_local.py` bridge and its versioned JSON contract. [Adapter bridge](../rust/tools/diarize_local.py) · [Rust adapter](../rust/src/bin/transcribe/attribution/adapter.rs)

1. Put FFmpeg on PATH.
2. Make a local Python 3.11 virtual environment.
3. Install the bridge's Python dependencies in that environment.
4. Download and extract `sherpa-onnx-pyannote-segmentation-3-0.tar.bz2`. [Segmentation model](https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2)
5. Download `nemo_en_titanet_small.onnx` into your model directory. [Speaker embedding model](https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/nemo_en_titanet_small.onnx)

```powershell
py -3.11 -m venv .\speaker-env
.\speaker-env\Scripts\python.exe -m pip install 'sherpa-onnx==2.0.3' numpy
```

Add an `adapter` object to your session manifest. Replace these example paths with the full paths on your computer. In the source tree, find the bridge in `rust/tools`. In the Windows ZIP, find it in `tools`.

```json
{
  "adapter": {
    "program": "C:/AudioTools/speaker-env/Scripts/python.exe",
    "args": [
      "C:/AudioTools/tools/diarize_local.py",
      "--segmentation", "C:/AudioTools/models/model.onnx",
      "--embedding", "C:/AudioTools/models/nemo_en_titanet_small.onnx"
    ],
    "timeout_seconds": 300
  }
}
```

Install the runtime and models before going offline. The bridge processes one source at a time on the CPU. It decodes the current source into memory. The default maximum is one hour of audio per source. Set `--max-seconds` in the adapter arguments to change this limit.

To configure a local adapter during guided preparation, use `--adapter EXE` and repeat `--adapter-arg ARG` for each argument. Use `--adapter-timeout N` to set the time limit in seconds.

Only put a program and arguments you trust in a manifest. Attribution runs that program with access to the selected audio. The supplied bridge loads models from disk and uses no endpoint.

## Attribute and review

Run the worker on one or more manifests.

```powershell
.\transcribe.exe attribute --session .\session.json
.\transcribe.exe review-session --session .\session.json
.\transcribe.exe review-session --session .\session.json --json
```

Repeat `--session` to process more than one session. Check each reported result. A partial revision can contain all recognized text even when a source or adapter fails.

Review shows speaker activity and its assigned text. Overlapping activity can contain several speaker labels. Text that crosses a speaker boundary can remain unresolved. The worker preserves each source's recognition text and tracks unresolved content separately from speech intervals.

Keep the manifest beside its `.audio-capture-sessions` directory. The worker stores numbered revisions there. Review commands use the latest revision and check for changes to the selected naming context.

### Review or infer a name

By default, review uses anonymous labels and shows name suggestions with evidence. Set `"automatic_names": true` in the manifest or use `--automatic-names` when you prepare it. Local English rules can use an explicit introduction in the first sentence of a turn. They also recognize supported directed questions and corresponding answers. An addressed name by itself is insufficient.

Separate tracks or provider-attributed turns can support an inferred name when the text and speaker assignment agree. If the local adapter alone groups voices on a single mixed track, names stay as suggestions. To apply a name, confirm the speaker identity or review the assignments of the turns that support it. Check the cited turns before you link an existing person ID.

Use `naming_context` to select approved supporting records and prior-session evidence. Verify the correspondence between speakers before you add a prior-session mapping. Keep those evidence files with the manifests. For example, this record can support a candidate that also has two clear, independently addressed exchanges.

```json
{
  "naming_context": {
    "roster": [
      {
        "record_id": "roster_review",
        "revision": 1,
        "name": "Nia",
        "text": "Nia hosts the review."
      }
    ]
  }
}
```

A prior transcript can provide a direct introduction. Add a `prior_context` entry in `naming_context` with the prior `session` manifest path, `current_speaker_id`, `prior_speaker_id`, `mapping_id`, and `mapping_revision`. Copy speaker IDs from the review output. A visible inferred name can change when its evidence changes.

During guided preparation, pass the context object from a JSON file with `--naming-context JSON`. Use the inner `naming_context` object as that file's root.

### Correct a revision

Copy the required IDs and latest revision from the review. Save changes in a UTF-8 patch file. This example corrects one turn and confirms a speaker's name.

```json
{
  "session_id": "s_demo",
  "expected_revision": 1,
  "operations": [
    {"op": "correct_text", "turn_id": "t_example", "text": "We will start at nine."},
    {"op": "confirm", "speaker_id": "sp_example", "name": "Nia"}
  ]
}
```

```powershell
.\transcribe.exe review-session --session .\session.json --apply .\edits.json
```

Use the new revision to review or export the correction. A stale patch is rejected. The earlier revision remains available locally.

## Export approved text

Save an export scope with the sessions and revisions to include. Add `turns` to select specific turn IDs. Omit it to select every turn. Use `redactions` to substitute approved text by turn ID. Provisional names require an explicit opt-in.

```json
{
  "schema_major": 1,
  "sessions": [
    {
      "manifest": "session.json",
      "revision": 2
    }
  ],
  "include_provisional_names": false
}
```

Check the selected revision and text first. Preview the scope before writing a batch.

```powershell
.\transcribe.exe export-memory --scope .\scope.json --output .\memory
.\transcribe.exe export-memory --scope .\scope.json --output .\memory --approve
```

The worker exports approved data as versioned JSONL. Audio, credentials, and local paths remain outside the export. Review the text for spoken names or other information you intend to redact.

When you enable provisional names, all supporting transcript citations for an exported inference must be in scope and unredacted. Exported evidence uses stable references to approved turns.

Use the same output stream for later approved revisions and scope changes. Submit an empty `sessions` array to withdraw the whole previous scope. A consumer must import the resulting batch to apply a withdrawal. Import batches in order. Replaying an identical batch leaves the consumer state unchanged.

Earlier published batches remain part of the stream's history. A redaction or withdrawal changes the next batch. To share only current approved text with a new recipient, use a fresh snapshot.

Copy the batch manifest path from the export receipt. The reference importer writes `state.json` in its own output directory.

```powershell
$batch = Read-Host 'Published batch manifest path'
.\transcribe.exe import-memory --batch $batch --output .\agent-memory
```

To resume, use the last cursor that the consumer imported. An earlier cursor returns the next committed batch. A cursor at the end of the stream previews pending changes. Add `--approve` to publish those changes.

```powershell
.\transcribe.exe export-memory --scope .\scope.json --output .\memory --after 12
```

For a new consumer, create a baseline from the current stream. Publish any pending changes to the scope first.

```powershell
.\transcribe.exe export-memory --scope .\scope.json --output .\memory --snapshot --approve
```

The reference importer refreshes its derived `messages` when the contributing text or identity changes. Use entity IDs and revisions to replace derived records in a separate memory system. Apply retractions to remove records from that system.

### Read the export from another tool

A committed batch contains `manifest.json` and `events.jsonl`. The manifest gives the schema version, stream ID, cursor, preceding cursor, event count, and payload hash.

An upsert replaces the current payload for the same session, entity type, and entity ID. A retraction withdraws that record. Apply a batch in one transaction after you check the version, hash, sequence, and previous cursor.

After you apply a correction, recalculate any search document or summary that used the replaced revision. After a retraction, remove material derived from the withdrawn record. Persist the cursor with that update. [Reference importer](../rust/src/bin/transcribe/attribution/importer.rs)

See the session format, naming rules, and memory requirements. [OpenSpec design](../openspec/changes/add-speaker-attribution-and-memory-export/design.md) · [Speaker requirements](../openspec/changes/add-speaker-attribution-and-memory-export/specs/speaker-attribution/spec.md) · [Export requirements](../openspec/changes/add-speaker-attribution-and-memory-export/specs/transcript-memory-export/spec.md)
