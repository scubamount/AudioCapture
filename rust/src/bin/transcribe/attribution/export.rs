use super::model::{self, Session, valid_id};
use super::{importer, store};
use crate::jobs;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Component, Path, PathBuf, Prefix};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    schema_major: u32,
    sessions: Vec<Selection>,
    #[serde(default)]
    include_provisional_names: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    manifest: PathBuf,
    revision: u64,
    turns: Option<BTreeSet<String>>,
    #[serde(default)]
    redactions: BTreeMap<String, String>,
}

impl Selection {
    fn includes(&self, id: &str) -> bool {
        self.turns.as_ref().is_none_or(|ids| ids.contains(id))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Batch {
    pub schema_major: u32,
    pub stream_id: String,
    pub snapshot: bool,
    pub predecessor: Option<u64>,
    pub cursor: u64,
    pub record_count: u64,
    pub sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Event {
    pub schema_major: u32,
    pub stream_id: String,
    pub seq: u64,
    pub event_id: String,
    pub session_id: String,
    pub kind: String,
    pub id: String,
    pub revision: u64,
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Index {
    stream_id: String,
    cursor: u64,
    // Keep withdrawn entity revisions. A later re-add must supersede its tombstone.
    entries: BTreeMap<String, Indexed>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Indexed {
    revision: u64,
    digest: Option<String>,
}

type Records = BTreeMap<String, Value>;
type VisibleTurns<'a> = BTreeMap<(&'a str, &'a str), (u64, &'a super::model::Turn)>;

pub(super) fn key(session: &str, kind: &str, id: &str) -> Result<String, String> {
    if !valid_id(session)
        || !valid_id(id)
        || !matches!(
            kind,
            "session" | "source" | "turn" | "activity" | "speaker" | "identity"
        )
    {
        return Err("Invalid memory entity identifier or kind".into());
    }
    Ok(format!("{session}/{kind}/{id}"))
}

pub(super) fn event_id(stream: &str, entity: &str, revision: u64) -> String {
    model::stable_id("event", &[stream, entity, &revision.to_string()])
}

pub(super) fn local_destination(path: &Path) -> Result<(), String> {
    fn drive(p: &Path) -> Result<u8, String> {
        match p.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => Ok(letter),
                _ => Err("Select a local drive for memory output".into()),
            },
            _ => Err("Cannot resolve the memory output drive".into()),
        }
    }
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    let letter = drive(&absolute)?;
    fn check(letter: u8) -> Result<(), String> {
        let root = [u16::from(letter), 58, 92, 0];
        // Windows returns DRIVE_REMOTE (4) for a mapped network drive.
        if unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) } == 4 {
            return Err("Select a local drive for memory output".into());
        }
        Ok(())
    }
    check(letter)?;
    // Check the resolved parent too: a local-looking junction can target a share.
    let parent = absolute
        .ancestors()
        .find(|p| p.exists())
        .ok_or("Cannot locate the destination drive")?;
    check(drive(&parent.canonicalize().map_err(|e| e.to_string())?)?)
}

fn insert(
    records: &mut Records,
    session: &str,
    kind: &str,
    id: &str,
    data: Value,
) -> Result<(), String> {
    if records.insert(key(session, kind, id)?, data).is_some() {
        return Err(format!("Duplicate {kind} in the export selection"));
    }
    Ok(())
}

fn vocabulary<'a>(value: &'a str, allowed: &[&str]) -> &'a str {
    if allowed.contains(&value) {
        value
    } else {
        "unknown"
    }
}

fn evidence_refs(speaker: &super::model::Speaker, visible: &VisibleTurns) -> Vec<Value> {
    let mut refs = BTreeSet::new();
    for entry in &speaker.evidence {
        if entry["confidence"] != "high"
            || entry["status"] != "inferred"
            || entry.get("candidate").and_then(Value::as_str) != speaker.display_name.as_deref()
        {
            continue;
        }
        let mut rule_refs = BTreeSet::new();
        let mut usable = true;
        for citation in entry["citations"].as_array().into_iter().flatten() {
            let (Some(sid), Some(tid)) = (
                citation["session_id"].as_str(),
                citation["turn_id"].as_str(),
            ) else {
                continue;
            };
            let current = visible.get(&(sid, tid));
            let start = citation["turn_span_bytes"][0]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok());
            let end = citation["turn_span_bytes"][1]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok());
            let (Some(&(rev, turn)), Some(start), Some(end), Some(text)) =
                (current, start, end, citation["text"].as_str())
            else {
                usable = false;
                break;
            };
            if citation["revision"].as_u64() != Some(rev)
                || citation["source_id"] != turn.source_id
                || citation["artifact_id"] != turn.artifact_id
                || turn
                    .reviewed_text
                    .as_ref()
                    .unwrap_or(&turn.raw_text)
                    .get(start..end)
                    != Some(text)
            {
                usable = false;
                break;
            }
            rule_refs.insert((sid, tid, rev));
        }
        if usable {
            refs.extend(rule_refs);
        }
    }
    refs.into_iter().map(|(session_id, turn_id, revision)| json!({ "session_id":session_id, "turn_id":turn_id, "revision":revision })).collect()
}

fn project(
    session: &Session,
    choice: &Selection,
    provisional: bool,
    visible: &VisibleTurns,
    previous: &Index,
    out: &mut Records,
) -> Result<Value, String> {
    let sid = &session.session_id;
    let all = choice.turns.is_none();
    let mut sources = BTreeSet::new();
    let mut speakers = BTreeSet::new();
    let mut selected = BTreeSet::new();
    let mut intervals: BTreeMap<(&str, &str), Vec<(u64, u64)>> = BTreeMap::new();
    for turn in session.turns.iter().filter(|t| choice.includes(&t.turn_id)) {
        let redacted = choice.redactions.get(&turn.turn_id);
        let text = redacted
            .or(turn.reviewed_text.as_ref())
            .unwrap_or(&turn.raw_text);
        let basis = if redacted.is_some() {
            "redacted"
        } else if turn.reviewed_text.is_some() {
            "reviewed"
        } else {
            "original"
        };
        if !valid_id(&turn.source_id)
            || !valid_id(&turn.artifact_id)
            || turn.speaker_ids.iter().any(|id| !valid_id(id))
        {
            return Err("Invalid source, artifact, or speaker identifier in a turn".into());
        }
        let parents: Vec<_> = turn
            .predecessors
            .iter()
            .filter(|id| key(sid, "turn", id).is_ok_and(|k| previous.entries.contains_key(&k)))
            .collect();
        insert(
            out,
            sid,
            "turn",
            &turn.turn_id,
            json!({
                "source_id":turn.source_id,"artifact_id":turn.artifact_id,"span_bytes":turn.span_bytes,
                "speaker_ids":turn.speaker_ids,"attribution":vocabulary(&turn.attribution,&["resolved","overlap","unresolved","unknown"]),
                "text":text,"text_basis":basis,"start_ms":turn.start_ms,"end_ms":turn.end_ms,
                "timing_method":vocabulary(&turn.timing_method,&["word","segment","words","aligned","untimed","provider","provider_word","provider_segment","chunk","unknown","none"]),
                "time_basis":"saved_recording","predecessors":parents
            }),
        )?;
        sources.insert(turn.source_id.as_str());
        speakers.extend(turn.speaker_ids.iter().map(String::as_str));
        selected.insert(turn.turn_id.as_str());
        if !all && let (Some(start), Some(end)) = (turn.start_ms, turn.end_ms) {
            for speaker in &turn.speaker_ids {
                intervals
                    .entry((&turn.source_id, speaker))
                    .or_default()
                    .push((start, end));
            }
        }
    }
    if choice
        .turns
        .as_ref()
        .is_some_and(|ids| ids.len() != selected.len())
        || choice
            .redactions
            .keys()
            .any(|id| !selected.contains(id.as_str()))
    {
        return Err("A selected or redacted turn does not exist in this approved revision".into());
    }

    // Prefix maxima answer containment queries without a scan of all approved turns.
    for ranges in intervals.values_mut() {
        ranges.sort_unstable();
        let mut max_end = 0;
        for (_, end) in ranges {
            max_end = max_end.max(*end);
            *end = max_end;
        }
    }
    for a in &session.activities {
        // A partial export must not disclose activity outside a selected time window.
        if !all
            && !intervals
                .get(&(a.source_id.as_str(), a.speaker_id.as_str()))
                .is_some_and(|ranges| {
                    let count = ranges.partition_point(|&(start, _)| start <= a.start_ms);
                    count > 0 && a.end_ms <= ranges[count - 1].1
                })
        {
            continue;
        }
        if !valid_id(&a.source_id) || !valid_id(&a.speaker_id) {
            return Err("Invalid activity reference".into());
        }
        sources.insert(a.source_id.as_str());
        speakers.insert(a.speaker_id.as_str());
        insert(
            out,
            sid,
            "activity",
            &a.activity_id,
            json!({"source_id":a.source_id,"speaker_id":a.speaker_id,
            "start_ms":a.start_ms,"end_ms":a.end_ms,"time_basis":"saved_recording"}),
        )?;
    }
    for source in session
        .sources
        .iter()
        .filter(|s| all || sources.contains(s.source_id.as_str()))
    {
        let mut data = json!({"role":vocabulary(&source.role,&["program","mic","microphone","system","mix","other","application","combined"]),
            "status":vocabulary(&source.status,&["success","ok","silent","untimed","partial","failed","missing"]),
            "time_basis":"saved_recording"});
        if valid_id(&source.artifact_id) {
            data["artifact_id"] = json!(source.artifact_id);
        }
        if all {
            data["duration_ms"] = json!(source.duration_ms);
            // Local diagnostic text can contain a private path or a provider response.
            data["gaps"] = Value::Array(
                source
                    .gaps
                    .iter()
                    .map(|gap| {
                        json!({
                            "kind":vocabulary(gap.get("kind").and_then(Value::as_str).unwrap_or("unknown"),
                                &["coverage","recognition","chunk","timing","missing","diarization"]),
                            "start_ms":gap.get("start_ms").and_then(Value::as_u64),
                            "end_ms":gap.get("end_ms").and_then(Value::as_u64)
                        })
                    })
                    .collect(),
            );
        }
        insert(out, sid, "source", &source.source_id, data)?;
    }
    for speaker in session
        .speakers
        .iter()
        .filter(|s| speakers.contains(s.speaker_id.as_str()))
    {
        let mut data = json!({"name_status":"anonymous"});
        if speaker.name_status == "confirmed" {
            if let (Some(name), Some(person)) = (&speaker.display_name, &speaker.person_id) {
                let person_key = key(sid, "identity", person)?;
                data = json!({"name_status":"confirmed","display_name":name,"person_id":person});
                let identity = json!({"display_name":name});
                if out
                    .insert(person_key, identity.clone())
                    .is_some_and(|old| old != identity)
                {
                    return Err("Conflicting approved names for one person in a session".into());
                }
            }
        } else if provisional && speaker.name_status == "inferred" && speaker.display_name.is_some()
        {
            let refs = evidence_refs(speaker, visible);
            if !refs.is_empty() {
                data = json!({"name_status":"inferred","display_name":speaker.display_name,"evidence_refs":refs});
            }
        }
        insert(out, sid, "speaker", &speaker.speaker_id, data)?;
    }
    insert(
        out,
        sid,
        "session",
        sid,
        json!({"coverage":if all {"full"} else {"partial"},
        "recognition_complete":session.complete,"shared_clock":session.shared_clock}),
    )?;
    Ok(
        json!({"session_id":sid,"revision":session.revision,"coverage":if all {"full"} else {"partial"},
        "selected_turns":selected.len(), "available_turns":session.turns.len(),"redacted_turns":choice.redactions.keys().collect::<Vec<_>>()}),
    )
}

pub(super) fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = File::open(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    serde_json::from_reader(BufReader::new(file))
        .map_err(|e| format!("Invalid {}: {e}", path.display()))
}

pub(super) fn write_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let temp = path.with_extension("pending");
    let mut writer = BufWriter::new(File::create(&temp).map_err(|e| e.to_string())?);
    serde_json::to_writer(&mut writer, value).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    writer.get_ref().sync_all().map_err(|e| e.to_string())?;
    drop(writer);
    fs::rename(&temp, path).map_err(|e| format!("Cannot commit {}: {e}", path.display()))
}

fn digest(value: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    jobs::digest(bytes.as_slice())
}

fn batches(output: &Path) -> Result<BTreeMap<u64, PathBuf>, String> {
    let entries = match fs::read_dir(output.join("batches")) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut batches = BTreeMap::new();
    for entry in entries {
        let e = entry.map_err(|e| e.to_string())?;
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(".stage-") {
            continue;
        }
        let cursor: u64 = name
            .parse()
            .map_err(|_| "Unexpected entry in memory batches directory")?;
        if name != format!("{cursor:020}") || !e.file_type().map_err(|e| e.to_string())?.is_dir() {
            return Err("Invalid published batch directory".into());
        }
        batches.insert(cursor, e.path());
    }
    Ok(batches)
}

fn load_index(output: &Path, history: &BTreeMap<u64, PathBuf>) -> Result<Index, String> {
    let Some((&tip, last)) = history.last_key_value() else {
        return Ok(Index::default());
    };
    let head: Batch = read(&last.join("manifest.json"))?;
    if head.cursor != tip || head.schema_major != 1 {
        return Err("Invalid published stream tip".into());
    }
    let cached = output.join(".publisher-index.json");
    if cached.is_file() {
        // This is a derived cache. A crash can leave it one commit behind.
        if let Ok(index) = read::<Index>(&cached) {
            if index.cursor == tip && index.stream_id == head.stream_id {
                return Ok(index);
            }
        }
    }
    let mut index = Index {
        stream_id: head.stream_id.clone(),
        ..Index::default()
    };
    for (&cursor, dir) in history {
        let header: Batch = read(&dir.join("manifest.json"))?;
        if header.stream_id != index.stream_id
            || header.cursor != cursor
            || header.predecessor
                != if index.cursor == 0 && header.snapshot {
                    None
                } else {
                    Some(index.cursor)
                }
        {
            return Err("Memory export history is incomplete".into());
        }
        importer::visit_events(dir, &header, |ev| {
            let k = key(&ev.session_id, &ev.kind, &ev.id)?;
            index.entries.insert(
                k,
                Indexed {
                    revision: ev.revision,
                    digest: ev.data.as_ref().map(digest).transpose()?,
                },
            );
            Ok(())
        })?;
        index.cursor = cursor;
    }
    Ok(index)
}

pub fn run(
    scope: &Path,
    output: &Path,
    after: Option<u64>,
    approve: bool,
) -> Result<String, String> {
    publish(scope, output, after, approve, false)
}

pub fn snapshot(scope: &Path, output: &Path, approve: bool) -> Result<String, String> {
    publish(scope, output, None, approve, true)
}

fn receipt(batch: &Path, header: &Batch, action: &str) -> String {
    json!({"action":action,"cursor":header.cursor,"stream_id":header.stream_id,"records":header.record_count,
        "batch":batch.join("manifest.json")}).to_string()
}

fn publish(
    scope_path: &Path,
    output: &Path,
    after: Option<u64>,
    approve: bool,
    baseline: bool,
) -> Result<String, String> {
    local_destination(output)?;
    let scope: Scope = read(scope_path)?;
    if scope.schema_major != 1 {
        return Err("Unsupported export scope schema".into());
    }
    let _guard = if approve {
        fs::create_dir_all(output).map_err(|e| e.to_string())?;
        Some(jobs::lock(&output.join(".publisher.lock"))?)
    } else {
        None
    };
    let history = batches(output)?;
    let mut index = load_index(output, &history)?;
    if let Some(cursor) = after {
        if cursor != index.cursor {
            if let Some((_next, dir)) = history
                .range((
                    std::ops::Bound::Excluded(cursor),
                    std::ops::Bound::Unbounded,
                ))
                .next()
            {
                let header: Batch = read(&dir.join("manifest.json"))?;
                if cursor == header.predecessor.unwrap_or(0) {
                    return Ok(receipt(dir, &header, "replay"));
                }
            }
            return Err(
                "Unknown or expired cursor. Request a fresh snapshot for this stream.".into(),
            );
        }
    }
    let root = scope_path.parent().unwrap_or(Path::new("."));
    let mut loaded = Vec::new();
    let mut records = Records::new();
    let mut coverage = Vec::new();
    let mut session_ids = BTreeSet::new();
    for selection in &scope.sessions {
        let session = store::for_export(&root.join(&selection.manifest))?;
        if session.schema_major != 1
            || selection.revision == 0
            || session.revision != selection.revision
        {
            return Err(
                "A selected session revision changed. Review the latest revision before export."
                    .into(),
            );
        }
        if !session_ids.insert(session.session_id.clone()) {
            return Err("Duplicate session in export selection".into());
        }
        if scope.include_provisional_names {
            loaded.push(session);
        } else {
            // Ordinary exports need only one raw session in memory at a time.
            coverage.push(project(
                &session,
                selection,
                false,
                &VisibleTurns::new(),
                &index,
                &mut records,
            )?);
        }
    }
    let mut visible = VisibleTurns::new();
    if scope.include_provisional_names {
        for (session, selection) in loaded.iter().zip(&scope.sessions) {
            for turn in &session.turns {
                if selection.includes(&turn.turn_id)
                    && !selection.redactions.contains_key(&turn.turn_id)
                {
                    visible.insert(
                        (&session.session_id, &turn.turn_id),
                        (session.revision, turn),
                    );
                }
            }
        }
    }
    for (session, selection) in loaded.iter().zip(&scope.sessions) {
        coverage.push(project(
            session,
            selection,
            scope.include_provisional_names,
            &visible,
            &index,
            &mut records,
        )?);
    }
    drop(visible);
    drop(loaded);
    let tip = index.cursor;
    let first = history.is_empty();
    if first && approve {
        index.stream_id = model::new_id("stream")?;
    }
    let mut changes = Vec::new();
    let mut count_up = 0;
    let mut count_withdraw = 0;
    let keys: BTreeSet<_> = index
        .entries
        .keys()
        .chain(records.keys())
        .cloned()
        .collect();
    for k in keys {
        let data = records.get(&k);
        let hash = data.map(digest).transpose()?;
        let prior = index.entries.get(&k);
        if hash.as_ref() == prior.and_then(|e| e.digest.as_ref()) {
            continue;
        }
        let revision = prior
            .map_or(0, |e| e.revision)
            .checked_add(1)
            .ok_or("Entity revision overflow")?;
        if data.is_some() {
            count_up += 1;
        } else {
            count_withdraw += 1;
        }
        index.entries.insert(
            k.clone(),
            Indexed {
                revision,
                digest: hash,
            },
        );
        changes.push((k, revision));
    }
    if baseline && !first && !changes.is_empty() {
        return Err(
            "Publish the approved selection before requesting its baseline snapshot".into(),
        );
    }
    if !approve {
        return Ok(json!({"action":"preview","cursor":tip,"snapshot":first||baseline,"coverage":coverage,
            "include_provisional_names":scope.include_provisional_names,"upserts":count_up,"retractions":count_withdraw}).to_string());
    }
    if !baseline && changes.is_empty() && !first {
        return Ok(
            json!({"action":"unchanged","cursor":tip,"stream_id":index.stream_id}).to_string(),
        );
    }
    if first && records.is_empty() {
        return Err("An initial snapshot requires an approved session".into());
    }
    let snapshot = first || baseline;
    let cursor = tip
        .checked_add(changes.len() as u64)
        .ok_or("Export cursor overflow")?;
    if baseline && !first {
        changes = records
            .keys()
            .map(|k| (k.clone(), index.entries[k].revision))
            .collect();
    }
    let parent = output.join(if baseline && !first {
        "snapshots"
    } else {
        "batches"
    });
    fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let final_dir = parent.join(format!("{cursor:020}"));
    if final_dir.exists() {
        let header: Batch = read(&final_dir.join("manifest.json"))?;
        importer::visit_events(&final_dir, &header, |_| Ok(()))?;
        return Ok(receipt(&final_dir, &header, "replay"));
    }
    let stage = parent.join(format!(".stage-{}", model::new_id("batch")?));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let result = (|| {
        let events = stage.join("events.jsonl");
        let mut writer = BufWriter::new(File::create(&events).map_err(|e| e.to_string())?);
        let count = changes.len() as u64;
        let start = cursor
            .checked_sub(count)
            .ok_or("Snapshot cursor underflow")?;
        for (offset, (k, revision)) in changes.into_iter().enumerate() {
            let mut parts = k.split('/');
            let seq = start + offset as u64 + 1;
            let data = records.remove(&k);
            let ev = Event {
                schema_major: 1,
                stream_id: index.stream_id.clone(),
                seq,
                event_id: event_id(&index.stream_id, &k, revision),
                session_id: parts.next().unwrap().into(),
                kind: parts.next().unwrap().into(),
                id: parts.next().unwrap().into(),
                revision,
                op: if data.is_some() { "upsert" } else { "retract" }.into(),
                data,
            };
            serde_json::to_writer(&mut writer, &ev).map_err(|e| e.to_string())?;
            writer.write_all(b"\n").map_err(|e| e.to_string())?;
        }
        writer.flush().map_err(|e| e.to_string())?;
        writer.get_ref().sync_all().map_err(|e| e.to_string())?;
        drop(writer);
        let header = Batch {
            schema_major: 1,
            stream_id: index.stream_id.clone(),
            snapshot,
            predecessor: if snapshot { None } else { Some(tip) },
            cursor,
            record_count: count,
            sha256: jobs::digest(File::open(events).map_err(|e| e.to_string())?)?,
        };
        write_atomic(&stage.join("manifest.json"), &header)?;
        // The directory rename publishes both the complete batch and its terminal cursor.
        fs::rename(&stage, &final_dir).map_err(|e| format!("Cannot commit memory batch: {e}"))?;
        if !baseline || first {
            index.cursor = cursor;
            // A failed cache update cannot roll back a published batch. Replay rebuilds it.
            let _ = write_atomic(&output.join(".publisher-index.json"), &index);
        }
        Ok(receipt(&final_dir, &header, "published"))
    })();
    if result.is_err() {
        // Delete only files this call created in its unique staging directory.
        for name in ["events.jsonl", "manifest.pending", "manifest.json"] {
            let _ = fs::remove_file(stage.join(name));
        }
        let _ = fs::remove_dir(stage);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::super::model::{Manifest, Source, Speaker, Turn};
    use super::*;
    struct Fixture {
        root: PathBuf,
        manifest: Manifest,
        session: Session,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(model::new_id("memory_export_test").unwrap());
            fs::create_dir(&root).unwrap();
            let manifest = Manifest {
                session_id: "chat".into(),
                automatic_names: true,
                ..Default::default()
            };
            write_atomic(&root.join("session.json"), &manifest).unwrap();
            let text = "My name is Nia.";
            let session = Session {
                schema_major: 1,
                naming_key: crate::attribution::name_context(&root, &manifest)
                    .unwrap()
                    .1,
                session_id: "chat".into(),
                revision: 1,
                shared_clock: true,
                complete: true,
                sources: vec![Source {
                    source_id: "input".into(),
                    role: "mic".into(),
                    status: "success".into(),
                    raw_text: text.into(),
                    artifact_id: "art".into(),
                    provenance: json!({"path":"secret-file","token":"secret-token"}),
                    gaps: vec![json!({"message":"secret-diagnostic"})],
                    ..Default::default()
                }],
                turns: vec![Turn {
                    turn_id: "a".into(),
                    source_id: "input".into(),
                    artifact_id: "art".into(),
                    raw_text: text.into(),
                    span_bytes: [0, text.len()],
                    speaker_ids: vec!["sp".into()],
                    attribution: "resolved".into(),
                    timing_method: "word".into(),
                    start_ms: Some(0),
                    end_ms: Some(1_000),
                    ..Default::default()
                }],
                speakers: vec![Speaker {
                    speaker_id: "sp".into(),
                    name_status: "inferred".into(),
                    display_name: Some("Nia".into()),
                    evidence: vec![json!({
                    "confidence":"high","status":"inferred","candidate":"Nia","citations":[{"session_id":"chat","turn_id":"a","revision":1,
                    "artifact_id":"art","source_id":"input","turn_span_bytes":[0,text.len()],"text":text}],"private":"secret-evidence"})],
                    ..Default::default()
                }],
                ..Default::default()
            };
            let fixture = Self {
                root,
                manifest,
                session,
            };
            fixture.save();
            fixture.scope(json!([{"manifest":"session.json","revision":1}]), false);
            fixture
        }
        fn save(&self) {
            store::update(&self.root.join("session.json"), &self.manifest, |_| {
                Ok(self.session.clone())
            })
            .unwrap();
        }
        fn scope(&self, sessions: Value, provisional: bool) {
            write_atomic(&self.root.join("scope.json"),&json!({"schema_major":1,"sessions":sessions,"include_provisional_names":provisional})).unwrap();
        }
        fn publish(
            &self,
            after: Option<u64>,
            approve: bool,
            baseline: bool,
        ) -> Result<Value, String> {
            let r = publish(
                &self.root.join("scope.json"),
                &self.root.join("stream"),
                after,
                approve,
                baseline,
            )?;
            serde_json::from_str(&r).map_err(|e| e.to_string())
        }
        fn import(&self, receipt: &Value) -> Result<Value, String> {
            importer::run(
                Path::new(receipt["batch"].as_str().unwrap()),
                &self.root.join("consumer"),
            )?;
            read(&self.root.join("consumer/state.json"))
        }
        fn payload(&self, r: &Value) -> String {
            fs::read_to_string(
                Path::new(r["batch"].as_str().unwrap()).with_file_name("events.jsonl"),
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn reviewed_scope_controls_exported_names_and_text() {
        let f = Fixture::new();
        let preview = f.publish(None, false, false).unwrap();
        assert_eq!(preview["coverage"][0]["selected_turns"], 1);
        assert!(!f.root.join("stream").exists());
        let first = f.publish(None, true, false).unwrap();
        let text = f.payload(&first);
        for denied in ["secret-", "evidence_refs", "display_name"] {
            assert!(!text.contains(denied));
        }
        assert!(text.contains("My name is Nia."));
        f.scope(json!([{"manifest":"session.json","revision":1}]), true);
        let naming = f.publish(None, true, false).unwrap();
        let payload = f.payload(&naming);
        assert!(payload.contains("evidence_refs"));
        assert!(!payload.contains("My name is Nia."));
        assert!(!payload.contains("secret-"));
        f.scope(
            json!([{"manifest":"session.json","revision":1,"redactions":{"a":"[redacted]"}}]),
            true,
        );
        let redact = f.publish(None, true, false).unwrap();
        let payload = f.payload(&redact);
        assert!(!payload.contains("Nia") && !payload.contains("secret-"));
        let _ = f.import(&first).unwrap();
        let named = f.import(&naming).unwrap();
        assert_eq!(
            named["messages"]["chat/turn/a"]["speakers"][0]["display_name"],
            "Nia"
        );
        let redacted = f.import(&redact).unwrap();
        assert_eq!(redacted["messages"]["chat/turn/a"]["text"], "[redacted]");
        assert!(!redacted.to_string().contains("Nia"));
        assert_eq!(f.session.turns[0].raw_text, "My name is Nia.");
    }

    #[test]
    fn stale_prior_evidence_cannot_publish_a_name() {
        let mut f = Fixture::new();
        f.session.revision = 2;
        f.session.turns[0].reviewed_text = Some("I spoke with Nia.".into());
        f.save();
        f.scope(json!([{"manifest":"session.json","revision":2}]), true);
        let first = f.publish(None, true, false).unwrap();
        let payload = f.payload(&first);
        assert!(!payload.contains("evidence_refs") && !payload.contains("display_name"));
        f.scope(json!([{"manifest":"session.json","revision":1}]), true);
        assert!(f.publish(None, true, false).is_err());
    }

    #[test]
    fn correction_split_and_withdrawal_update_derived_messages_atomically() {
        let mut f = Fixture::new();
        let first = f.publish(None, true, false).unwrap();
        let mut before = f.import(&first).unwrap();
        assert_eq!(
            before["messages"]["chat/turn/a"]["text"],
            f.session.turns[0].raw_text
        );
        f.session.revision = 2;
        let base = f.session.turns.pop().unwrap();
        for (name, text, span) in [("b", "My name ", [0, 8]), ("c", "is Nia.", [8, 15])] {
            f.session.turns.push(Turn {
                turn_id: name.into(),
                raw_text: text.into(),
                span_bytes: span,
                predecessors: vec!["a".into()],
                ..base.clone()
            });
        }
        f.session.speakers[0].name_status = "confirmed".into();
        f.session.speakers[0].person_id = Some("person1".into());
        f.save();
        f.scope(json!([{"manifest":"session.json","revision":2}]), false);
        let split = f
            .publish(Some(first["cursor"].as_u64().unwrap()), true, false)
            .unwrap();
        let changed = f.import(&split).unwrap();
        assert!(changed["messages"].get("chat/turn/a").is_none());
        assert_eq!(
            changed["entities"]["chat/turn/b"]["data"]["predecessors"],
            json!(["a"])
        );
        assert_eq!(
            changed["messages"]["chat/turn/b"]["speakers"][0]["display_name"],
            "Nia"
        );
        assert!(
            !changed["messages"]["chat/turn/b"]["dependencies"]["chat/identity/person1"].is_null()
        );
        before = f.import(&split).unwrap();
        assert_eq!(before, changed);
        f.scope(json!([]), false);
        let withdrawn = f.publish(None, true, false).unwrap();
        let gone = f.import(&withdrawn).unwrap();
        assert_eq!(gone["messages"], json!({}));
        assert!(
            gone["entities"]
                .as_object()
                .unwrap()
                .values()
                .all(|e| e["data"].is_null())
        );
        let successor = f.publish(first["cursor"].as_u64(), false, false).unwrap();
        assert_eq!(successor["batch"], split["batch"]);
    }

    #[test]
    fn malformed_gap_and_content_conflicts_leave_checkpoint_untouched() {
        let f = Fixture::new();
        let first = f.publish(None, true, false).unwrap();
        let state = f.import(&first).unwrap();
        f.scope(json!([]), false);
        let withdraw = f.publish(None, true, false).unwrap();
        let path = PathBuf::from(withdraw["batch"].as_str().unwrap());
        let mut header: Batch = read(&path).unwrap();
        let original = header.clone();
        header.schema_major = 2;
        write_atomic(&path, &header).unwrap();
        assert!(f.import(&withdraw).is_err());
        header = original.clone();
        header.predecessor = Some(123);
        write_atomic(&path, &header).unwrap();
        assert!(f.import(&withdraw).is_err());
        write_atomic(&path, &original).unwrap();
        let events = path.with_file_name("events.jsonl");
        let saved = fs::read(&events).unwrap();
        fs::write(&events, b"{}\n").unwrap();
        assert!(f.import(&withdraw).is_err());
        let recorded: Value = read(&f.root.join("consumer/state.json")).unwrap();
        assert_eq!(recorded, state);
        fs::write(&events, saved).unwrap();
        assert_eq!(f.import(&withdraw).unwrap()["messages"], json!({}));
    }

    #[test]
    fn conflicting_revision_cannot_replace_a_live_record() {
        let f = Fixture::new();
        let first = f.publish(None, true, false).unwrap();
        let before = f.import(&first).unwrap();
        f.scope(json!([{"manifest":"session.json","revision":1}]), true);
        let next = f.publish(None, true, false).unwrap();
        let path = PathBuf::from(next["batch"].as_str().unwrap());
        let eventfile = path.with_file_name("events.jsonl");
        let mut event: Event =
            serde_json::from_str(&fs::read_to_string(&eventfile).unwrap()).unwrap();
        event.revision = 1;
        event.event_id = event_id(
            &event.stream_id,
            &key(&event.session_id, &event.kind, &event.id).unwrap(),
            1,
        );
        let mut bytes = serde_json::to_vec(&event).unwrap();
        bytes.push(b'\n');
        let mut header: Batch = read(&path).unwrap();
        header.sha256 = jobs::digest(bytes.as_slice()).unwrap();
        fs::write(eventfile, bytes).unwrap();
        write_atomic(&path, &header).unwrap();
        assert!(f.import(&next).unwrap_err().contains("revision"));
        let after: Value = read(&f.root.join("consumer/state.json")).unwrap();
        assert_eq!(after, before);
    }

    #[test]
    fn snapshot_withdrawal_reconciliation_survives_multiple_missed_revisions() {
        let f = Fixture::new();
        let all = json!([{"manifest":"session.json","revision":1}]);
        let first = f.publish(None, true, false).unwrap();
        f.import(&first).unwrap();
        for _ in 0..2 {
            f.scope(json!([]), false);
            f.publish(None, true, false).unwrap();
            f.scope(all.clone(), false);
            f.publish(None, true, false).unwrap();
        }
        f.scope(json!([]), false);
        let withdrawal = f.publish(None, true, false).unwrap();
        // A process may exit after publication, before rebuilding its optional index.
        fs::remove_file(f.root.join("stream/.publisher-index.json")).unwrap();
        fs::create_dir(f.root.join("stream/batches/.stage-interrupted")).unwrap();
        let empty = f.publish(None, true, true).unwrap();
        assert_eq!(empty["cursor"], withdrawal["cursor"]);
        assert_eq!(f.import(&empty).unwrap()["messages"], json!({}));
        f.scope(all, false);
        let restored = f.publish(None, true, false).unwrap();
        let state = f.import(&restored).unwrap();
        assert_eq!(state["entities"]["chat/turn/a"]["revision"], 7);
        assert_eq!(state["messages"]["chat/turn/a"]["text"], "My name is Nia.");
        assert!(f.publish(Some(1), true, false).is_err());
    }
}
