## Inputs and execution

The current worker stores per-part job state under `.audio-capture-jobs`. Successful jobs point to text and optional provider JSON. Chunk aggregates retain source hashes, frame offsets, rates, part text, and segments. HTML review exports a separate correction file. Use these outputs as inputs. Preserve original media, recognition text, provider responses, and earlier review decisions.

The following interfaces are implementation targets. Existing scripted and guided commands retain their defaults.

| Proposed worker operation | Input and result |
| --- | --- |
| `attribute --session <manifest> [--session <manifest> ...]` | Process one or more explicitly selected sessions. Write versioned local transcript revisions. |
| `review-session --session <manifest>` | Show source, time, text, anonymous labels, and evidence. Record local edits and identity decisions. |
| `export-memory --scope <selection> --output <directory> [--after <cursor>]` | Publish a local snapshot or subsequent approved changes. Replay a saved cursor after interruption. |

The manifest identifies finalized inputs, job references, canonical tracks, time maps, and optional context transcript IDs. Guided selection creates it after the operator reviews track grouping. A batch provides the same selections explicitly. Each item has an independent result. Seeing a new file, starting capture, or finishing capture triggers no analysis, upload, or export.

### Diarization and timing

Prefer separate sources for a confirmed session. If only the mix exists, analyze the mix. Require a representation choice for ambiguous or partial sets. Never join unrelated sessions because file names resemble each other. Keep the choice in the manifest. A chunk aggregate and its parts constitute one transcription input.

Support an explicitly configured, local subprocess adapter. Supply the selected local audio reference, source ID, decoded duration, and protocol version through UTF-8 JSON on stdin. Require a versioned JSON result with backend/model versions, speaker labels, and source-relative `start_ms` and `end_ms`. Concurrent intervals represent overlap. Reject non-finite, reversed, negative, or out-of-range intervals. Bound adapter runtime, output size, and child concurrency. Record timeout, error, or missing model per source. Model installation and selection are explicit. The offline path requires no remote service, credential, model download, or language model.

Existing ASR may use the configured OpenAI-compatible endpoint. Send any optional diarization format only for an explicitly selected endpoint/model profile with a known response contract. Record the effective profile and schema version. Unknown profiles keep existing request fields. Treat malformed speaker labels or intervals as unavailable. Preserve usable text. Do not retry uploads to acquire labels. A separate naming service would need its own explicit choice of destination and context; the baseline uses local evidence rules.

Store integer milliseconds on the saved recording timeline. Apply the worker's chunk frame offsets once, before aligning timed words or segments. Normalize chunk-local labels only with supported continuity evidence. Use a new anonymous speaker when continuity is uncertain. For a timed span, assign the unique supported active speaker. Keep concurrent candidates on ambiguous spans. A coarse segment crossing a speaker boundary retains its text once as unresolved unless word alignment supports a split. Preserve speaker activity separately from text attribution. An untimed result can gain word or segment times through an explicitly selected local aligner. Without alignment, keep the full text untimed. Original chunk bounds may identify its coverage. Keep unknown wall time null after pause or silence removal without a verified time map.

### Identity decisions

Allocate independent opaque IDs for sessions, sources, anonymous speakers, turns, evidence, and reviewed people. Keep speaker IDs scoped to a session. Source names, model cluster numbers, name strings, and timestamps are unsuitable primary keys. Register each source once per session and verify bytes against a local digest. A saved manifest carries the IDs across renames and replays. Replaced bytes require a new source or explicit reconciliation.

Resolve evidence only from operator-selected transcripts or confirmed roster/alias records. Start with deterministic local rules for explicit, unquoted first-person introductions and corroborated exchanges. Keep candidate name, evidence type, exact text span, cited turn and revision, rule version, and confidence tier. An unambiguous direct introduction on a uniquely attributed turn qualifies for high confidence if no evidence conflicts. Exclude quoted or reported introductions, hypotheticals, negations, and third-party mentions from self-identification. Two speakers can share a name.

The second high-confidence rule uses two disjoint, directed exchanges. For example, “Nia, can you send the agenda?” receives “I'll send the agenda.” Another speaker later asks “Nia, will you host the review?” The same anonymous speaker answers “I'll host the review.” Two different speakers must make the requests. A single respondent must answer each request explicitly, with no overlapping or competing reply. An independent, permitted source must corroborate the participant or role, such as a reviewed meeting record assigning Nia the review. Do not count repeated ASR outputs or quotations of one exchange as independent evidence. Store each request, reply, corroborating span, and any conflict. A roster name or mere turn adjacency cannot satisfy this rule.

Default review displays anonymous labels with suggestions. Optional automatic display promotes either supported high-confidence rule to an *inferred* name without per-speaker approval. Cross-session person links require review. Confirm, reject, rename, merge, unlink, and split by recording local decisions. Preserve anonymous speaker IDs. Changes to evidence invalidate dependent inferences pending re-evaluation. A reviewer can relink the correct person and replay the revision history.

Approved prior transcripts can supply a direct introduction. Require a reviewed correspondence or the same stable participant key from an approved, verified mapping of speakers to participants. This permits an inferred display name without another name prompt. Cite the prior introduction and mapping on the current speaker. An ordinary Discord process track supplies no participant key. Never infer correspondence from a repeated source label or a matching name alone. Neither a persistent voiceprint registry nor voice synthesis is part of this design.

### Transcript and export format

Store a versioned UTF-8 JSON session document. A session revision carries these data:

| Record | Required fields or semantics |
| --- | --- |
| Session | Schema version, session ID, revision, completeness, selected source IDs, alignment basis, and per-source status. |
| Source | Source ID, role, recording time basis, duration if known, raw artifact references, ASR run/adapter provenance, and missing ranges or failures. Keep local paths in local storage. |
| Turn or untimed text | Stable turn ID, source ID, chunk or artifact reference, original text span, reviewed text if changed, start/end or null, timing method, speaker IDs, and attribution status. |
| Speaker activity | Source ID, interval, session-local speaker ID or unknown, backend version, and overlap membership independent of any aligned words. |
| Evidence or decision | Stable ID, affected speaker/turn IDs, confidence tier, source spans, revision, and any superseded decision. Confirmed person IDs remain distinct from display names. |

Anchor source spans with artifact ID and half-open UTF-8 byte offsets. Store separators or intervening spans to reproduce every canonical source text exactly. Overlapping speech can share a clock interval. Each text span appears once in reconstruction. If provider segments cannot be matched safely, keep full source text untimed. For a shared clock, order timed entries by start, source ID, then turn ID. Group untimed text by source and source order. On unrelated clocks keep separate timelines. Completeness means all selected recognition output is accounted for. Flag missing sources, chunks, or recognition failures explicitly. The document makes no claim to recover speech missed by ASR.

Local review selects sessions, turns, exclusions, and redactions before export. Export is off by default. Preserve locally complete transcripts when exporting a subset. The preview identifies scope, omissions, and revision. Raw audio, embeddings, credentials, endpoint URLs, local paths, and unapproved context stay local. Exclude unconfirmed person IDs, aliases, and inferred-name metadata by default. Spoken names in approved transcript text remain visible unless redacted. Exporting provisional labels requires a separate opt-in. Such labels include status and permitted evidence references. Evidence outside export scope remains private.

Use one named export stream per destination and review selection. Keep that stream's ID when the operator changes its selection. Publish UTF-8 JSONL batches with an atomic manifest containing schema major version, stream ID, predecessor cursor, terminal cursor, record count, and content digest. A baseline snapshot starts a new consumer at a declared cursor. Delta batches chain from the last acknowledged cursor. Each event carries stream ID, monotonic sequence, event ID, session ID, entity kind/ID, revision, and operation. An upsert carries the approved complete replacement payload. A retract carries only identifiers and revision in its envelope. Export approved session, source, activity, speaker, turn, and optional identity records. Keep source lineage resolvable using scoped opaque references.

These illustrative events belong to successive batches. A concurrent speaker can have another activity interval over the same milliseconds.

```jsonl
{"schema_major":1,"stream_id":"feed1","seq":9,"event_id":"e9","session_id":"s1","kind":"turn","id":"t4","revision":1,"op":"upsert","data":{"source_id":"a1","artifact_id":"r1","span_bytes":[0,4],"speaker_ids":["sp2"],"attribution":"resolved","text":"Yes.","start_ms":1200,"end_ms":1460,"time_basis":"saved_recording","timing_method":"aligned"}}
{"schema_major":1,"stream_id":"feed1","seq":10,"event_id":"e10","session_id":"s1","kind":"turn","id":"t4","revision":2,"op":"retract"}
```

Commit each batch and its new cursor together. A consumer validates digest, count, version, predecessor, and consecutive sequence before changing state. Apply atomically by stream and entity ID. Replaying an identical batch has no effect. Reject conflicts for the same revision, missing sequences, and unsupported major versions without advancing the cursor. Handle retries under a single writer lock. An abandoned temporary batch remains invisible. Text edits retain entity IDs and increase revisions. Splits and merges retract replaced turns and upsert successors in the same batch with lineage. Redaction replaces exported text. Withdrawal or scope reduction issues retractions to the same stream for already exported entities. Retain tombstones or require a fresh snapshot when a cursor expires. Mark expired cursors explicitly. A compliant importer invalidates dependent derived memories by source entity/revision and recomputes them from approved replacements. Retraction takes effect when the destination imports it.

### Planned acceptance

Use fixed, synthetic voices with reference transcripts, speaker roles, boundaries, overlap, identities, and source span mappings. Keep test sessions separate from tuning sessions. Include at least twelve held-out sessions, three synthetic voices per session, and six sessions with overlapping speech. Include mix-plus-stems, two Discord speakers, a separate microphone, chunk boundaries, short turns, and quiet speech. Include two sessions with the same display name, contradictory introductions, and later corrections.

Measure diarization error as missed, spurious, and misassigned speaker time divided by reference speaker time. Score with optimal per-session speaker mapping, overlap included, and a 250 ms boundary collar. Target pooled error at most 20% and isolated-turn recall at least 90% for reference turns lasting one second or more. Count a turn as recovered when the mapped speaker covers at least half its duration. Publish per-session scores and overlap-only errors. These are proposed gates for a named backend on this fixture. Natural conversations need separate validation.

Use at least 20 eligible introductions, 20 eligible corroborated conversational matches, and 40 negative naming examples across the corpus. Include single addressed mentions, overlapping replies, conflicting names, stale evidence, and copies of one exchange among the negatives. Score each naming rule separately with reference turns and speaker labels, then repeat using the actual backend. Require at least 90% recall for introductions and 80% for corroborated conversational matches on the reference inputs. Require zero incorrect automatic names and zero automatic cross-session confirmations in both evaluations. Require exact source text reconstruction and identical consumer state after every replay, correction, split, retraction, and recovery fixture. Report the backend/model versions, corpus hashes, raw counts, elapsed time, and peak memory. A missing backend yields an untested result.
