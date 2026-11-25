use super::export::{self, Batch, Event, key};
use super::model::valid_id;
use crate::jobs;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Seek};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

// entity records include tombstones. The revision protects a deleted record from stale replays.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    revision: u64,
    data: Option<Value>,
    // An omitted snapshot entity may have changed several times before withdrawal.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    revision_is_floor: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Consumer {
    schema_major: u32,
    stream_id: String,
    cursor: u64,
    #[serde(default)]
    partial_history: bool,
    entities: BTreeMap<String, Entry>,
    messages: BTreeMap<String, Value>,
    // A digest lets a resumed consumer recognize exact old batches after later changes.
    applied: BTreeMap<String, String>,
}

fn validate_header(h: &Batch) -> Result<(), String> {
    if h.schema_major != 1
        || !valid_id(&h.stream_id)
        || h.cursor == 0
        || h.sha256.len() != 64
        || !h.sha256.bytes().all(|x| x.is_ascii_hexdigit())
    {
        return Err("Invalid or unsupported memory batch manifest".into());
    }
    if h.snapshot {
        if h.predecessor.is_some() || h.record_count > h.cursor {
            return Err("Invalid snapshot cursor".into());
        }
    } else if h.record_count == 0
        || h.predecessor.and_then(|p| p.checked_add(h.record_count)) != Some(h.cursor)
    {
        return Err("Invalid delta cursor or count".into());
    }
    Ok(())
}

pub(super) fn visit_events(
    dir: &Path,
    h: &Batch,
    mut visit: impl FnMut(Event) -> Result<(), String>,
) -> Result<(), String> {
    validate_header(h)?;
    // The sharing mode prevents a writer from replacing the payload between hashing and reading.
    let mut f = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(dir.join("events.jsonl"))
        .map_err(|e| e.to_string())?;
    if jobs::digest(&mut f)? != h.sha256 {
        return Err("Memory batch content digest mismatch".into());
    }
    f.rewind().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(f);
    let mut line = String::new();
    let mut seq = h.cursor - h.record_count;
    let mut seen = BTreeSet::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            break;
        }
        let e: Event =
            serde_json::from_str(&line).map_err(|e| format!("Invalid memory event: {e}"))?;
        seq = seq.checked_add(1).ok_or("Memory event sequence overflow")?;
        let entity = key(&e.session_id, &e.kind, &e.id)?;
        if e.schema_major != 1
            || e.revision == 0
            || e.stream_id != h.stream_id
            || e.seq != seq
            || e.event_id != export::event_id(&h.stream_id, &entity, e.revision)
            || seq > h.cursor
            || !matches!(
                (&*e.op, &e.data),
                ("upsert", Some(Value::Object(_))) | ("retract", None)
            )
            || (h.snapshot && e.op != "upsert")
        {
            return Err("Invalid memory event envelope, sequence, or operation".into());
        }
        if !seen.insert(entity) {
            return Err("Duplicate entity in memory batch".into());
        }
        visit(e)?;
    }
    if seq != h.cursor {
        return Err("Memory batch record count mismatch".into());
    }
    Ok(())
}

fn refresh_messages(
    entities: &BTreeMap<String, Entry>,
    messages: &mut BTreeMap<String, Value>,
    changed: &BTreeSet<String>,
) -> Result<(), String> {
    let mut affected: BTreeSet<_> = changed
        .iter()
        .filter(|key| key.split('/').nth(1) == Some("turn"))
        .cloned()
        .collect();
    for (key, message) in messages.iter() {
        let deps = message["dependencies"]
            .as_object()
            .ok_or("Stored message lacks dependency revisions")?;
        if deps.keys().any(|dep| changed.contains(dep)) {
            affected.insert(key.clone());
        }
    }
    for k in &affected {
        messages.remove(k);
        let Some(turn_entry) = entities.get(k) else {
            continue;
        };
        let Some(turn) = turn_entry.data.as_ref() else {
            continue;
        };
        let mut parts = k.split('/');
        let sid = parts.next().ok_or("Invalid stored entity key")?;
        if parts.next() != Some("turn") {
            continue;
        }
        let source = turn["source_id"]
            .as_str()
            .ok_or("Turn is missing its source")?;
        let source_key = key(sid, "source", source)?;
        let session_key = key(sid, "session", sid)?;
        let mut deps = BTreeMap::new();
        for (entity, rec) in [
            (k.clone(), Some(turn_entry)),
            (session_key.clone(), entities.get(&session_key)),
            (source_key.clone(), entities.get(&source_key)),
        ] {
            let rec = rec
                .filter(|v| v.data.is_some())
                .ok_or("Turn references a missing approved session or source")?;
            deps.insert(entity, rec.revision);
        }
        let mut speakers = Vec::new();
        for id in turn["speaker_ids"]
            .as_array()
            .ok_or("Turn is missing its speaker list")?
        {
            let id = id.as_str().ok_or("Invalid speaker identifier")?;
            let speaker_key = key(sid, "speaker", id)?;
            let entry = entities
                .get(&speaker_key)
                .ok_or("Turn references a missing speaker")?;
            let speaker = entry
                .data
                .as_ref()
                .ok_or("Turn references a withdrawn speaker")?;
            deps.insert(speaker_key, entry.revision);
            if let Some(person) = speaker["person_id"].as_str() {
                let person_key = key(sid, "identity", person)?;
                let identity = entities
                    .get(&person_key)
                    .filter(|e| e.data.is_some())
                    .ok_or("Speaker references a withdrawn identity")?;
                deps.insert(person_key, identity.revision);
            }
            speakers.push(json!({"speaker_id":id,"display_name":speaker["display_name"],"name_status":speaker["name_status"]}));
        }
        let text = turn["text"]
            .as_str()
            .ok_or("Turn is missing its approved text")?;
        messages.insert(k.clone(),json!({"session_id":sid,"turn_id":parts.next(),"source_id":source,
            "artifact_id":turn["artifact_id"],"span_bytes":turn["span_bytes"],"text":text,"text_basis":turn["text_basis"],
            "start_ms":turn["start_ms"],"end_ms":turn["end_ms"],"time_basis":turn["time_basis"],
            "attribution":turn["attribution"],"speakers":speakers,"dependencies":deps}));
    }
    Ok(())
}

fn check_snapshot(
    state: &Consumer,
    next: &BTreeMap<String, Entry>,
    h: &Batch,
) -> Result<(), String> {
    if h.cursor != state.cursor {
        return Ok(());
    }
    for (k, record) in state.entities.iter().filter(|(_, e)| e.data.is_some()) {
        if !next
            .get(k)
            .is_some_and(|e| e.revision == record.revision && e.data == record.data)
        {
            return Err("Snapshot conflicts with the consumer at this cursor".into());
        }
    }
    if next.values().filter(|e| e.data.is_some()).count()
        != state.entities.values().filter(|e| e.data.is_some()).count()
    {
        return Err("Snapshot changes the entity set at this cursor".into());
    }
    Ok(())
}

fn batch_dir(batch: &Path) -> Result<PathBuf, String> {
    if batch.is_dir() {
        return Ok(batch.to_owned());
    }
    if batch.file_name().is_some_and(|n| n == "manifest.json") {
        return Ok(batch.parent().unwrap_or(Path::new(".")).to_owned());
    }
    Err("Select a memory batch directory or its manifest.json".into())
}

pub fn run(batch: &Path, output: &Path) -> Result<String, String> {
    export::local_destination(output)?;
    let dir = batch_dir(batch)?;
    let h: Batch = export::read(&dir.join("manifest.json"))?;
    validate_header(&h)?;
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let _guard = jobs::lock(&output.join(".consumer.lock"))?;
    let file = output.join("state.json");
    let mut state: Consumer = if file.exists() {
        export::read(&file)?
    } else {
        Consumer {
            schema_major: 1,
            stream_id: h.stream_id.clone(),
            ..Consumer::default()
        }
    };
    if state.schema_major != 1 || state.stream_id != h.stream_id {
        return Err("Consumer state has a different schema or stream".into());
    }
    let batch_id = format!(
        "{}-{}-{}",
        if h.snapshot { "snapshot" } else { "delta" },
        h.predecessor.unwrap_or(0),
        h.cursor
    );
    if let Some(digest) = state.applied.get(&batch_id) {
        if digest != &h.sha256 {
            return Err("Conflicting replay of a previously applied batch".into());
        }
        visit_events(&dir, &h, |_| Ok(()))?;
        return Ok(
            json!({"action":"replay","cursor":state.cursor,"stream_id":state.stream_id})
                .to_string(),
        );
    }
    if h.snapshot {
        if h.cursor < state.cursor {
            return Err("Stale snapshot would rewind consumer state".into());
        }
    } else if state.cursor == 0 || h.predecessor != Some(state.cursor) {
        return Err(
            "Missing predecessor. Import the intervening batches or a fresh snapshot.".into(),
        );
    }
    let mut replacements = BTreeMap::new();
    visit_events(&dir, &h, |e| {
        let k = key(&e.session_id, &e.kind, &e.id)?;
        if let Some(old) = state.entities.get(&k) {
            if e.revision < old.revision
                || (e.revision == old.revision && (old.data != e.data || !h.snapshot))
            {
                return Err("Conflicting or stale entity revision".into());
            }
            if !h.snapshot
                && !old.revision_is_floor
                && old.revision.checked_add(1) != Some(e.revision)
            {
                return Err("Skipped entity revision in a delta".into());
            }
            if !h.snapshot && old.data.is_none() && e.data.is_none() {
                return Err("Cannot withdraw an entity twice".into());
            }
        } else if !h.snapshot && (e.data.is_none() || (!state.partial_history && e.revision != 1)) {
            return Err("Unexpected withdrawal or initial revision for an unknown entity".into());
        }
        replacements.insert(
            k,
            Entry {
                revision: e.revision,
                data: e.data,
                revision_is_floor: false,
            },
        );
        Ok(())
    })?;
    let mut changed: BTreeSet<String> = replacements.keys().cloned().collect();
    if h.snapshot {
        check_snapshot(&state, &replacements, &h)?;
        // Reconcile IDs absent from this baseline, even if the consumer missed the withdrawal.
        for (k, entry) in &mut state.entities {
            if !replacements.contains_key(k) && h.cursor > state.cursor {
                if entry.data.is_some() {
                    changed.insert(k.clone());
                }
                entry.data = None;
                entry.revision_is_floor = true;
            }
        }
        state.partial_history |= h.cursor > h.record_count;
    }
    state.entities.append(&mut replacements);
    // Derive only from the new approved graph; an old label or withdrawn passage cannot survive.
    refresh_messages(&state.entities, &mut state.messages, &changed)?;
    state.applied.insert(batch_id, h.sha256);
    state.cursor = h.cursor;
    export::write_atomic(&file, &state)?;
    Ok(json!({"action":"imported","cursor":state.cursor,"stream_id":state.stream_id,"messages":state.messages.len()}).to_string())
}
