//! Import a completed live interval without replaying recognition.
#[cfg(test)]
mod tests;
mod timeline;
use super::model::{Manifest, SCHEMA_MAJOR, Session, Source, Speaker, Turn, stable_id};
use super::{export, store};
use crate::jobs;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

type Result<T> = std::result::Result<T, String>;
type Media = (PathBuf, PathBuf, PathBuf);
type Range = (u64, u64);
fn pinned(path: &Path) -> Result<File> {
    export::local_destination(path)?;
    let f = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .map_err(|e| format!("Cannot open selected file: {e}"))?;
    if !f.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Select a regular file.".into());
    }
    Ok(f)
}
fn hash(file: &mut File) -> Result<String> {
    let value = jobs::digest(&mut *file)?;
    file.rewind().map_err(|e| e.to_string())?;
    Ok(value)
}
fn lines(file: &File, mut consume: impl FnMut(Value) -> Result<()>) -> Result<()> {
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = (&mut reader)
            .take(4 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut buf)
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if n > 4 * 1024 * 1024 || buf.last() != Some(&b'\n') {
            return Err("Oversized or unfinished index record.".into());
        }
        consume(serde_json::from_slice(&buf).map_err(|e| format!("Invalid index record: {e}"))?)?;
    }
    Ok(())
}
fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| format!("Missing text field {key}"))
}
fn number(v: &Value, key: &str) -> Result<u64> {
    v[key]
        .as_u64()
        .ok_or_else(|| format!("Missing integer field {key}"))
}
fn range(v: &Value, a: &str, b: &str) -> Result<Range> {
    let pair = (number(v, a)?, number(v, b)?);
    if pair.1 < pair.0 {
        return Err("Reversed frame range.".into());
    }
    Ok(pair)
}
#[derive(Default)]
struct Index {
    session: Session,
    capture: String,
    interval: String,
    start: u64,
    labels: Vec<String>,
    recordings: BTreeMap<String, Range>,
    bounds: BTreeMap<String, Range>,
}
fn speaker(session: &mut Session, known: &mut BTreeSet<String>, id: String) -> String {
    if known.insert(id.clone()) {
        session.speakers.push(Speaker {
            speaker_id: id.clone(),
            name_status: "anonymous".into(),
            ..Default::default()
        })
    }
    id
}
fn parse(file: &File, digest: &str) -> Result<Index> {
    let mut index = Index::default();
    let mut sequence = 0;
    let mut sealed = false;
    let mut turn_indices = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut max_frame = 0;
    let mut counts = (0u64, 0u64);
    let mut source_lookup = BTreeMap::new();
    lines(file, |v| {
        sequence += 1;
        let kind = string(&v, "type")?;
        if sealed || number(&v, "sequence")? != sequence || (sequence == 1) != (kind == "begin") {
            return Err("Invalid order in live segment index.".into());
        }
        if kind == "begin" {
            if number(&v, "schema")? != 1 {
                return Err("Unsupported live index version.".into());
            }
            index.capture = string(&v, "session")?.to_owned();
            index.interval = string(&v, "interval")?.to_owned();
            index.start = number(&v, "start_frame")?;
            max_frame = index.start;
            let id = stable_id("live", &[&index.capture, &index.interval]);
            index.session = Session {
                schema_major: SCHEMA_MAJOR,
                session_id: id.clone(),
                shared_clock: true,
                revision: 1,
                analysis_id: digest.to_owned(),
                ..Default::default()
            };
            index.labels = serde_json::from_value(v["sources"].clone())
                .map_err(|_| "Index needs ordered source labels.")?;
            if index.labels.is_empty() || index.labels.len() > 128 {
                return Err("Invalid live source count.".into());
            }
            for (i, label) in index.labels.iter().enumerate() {
                if label.is_empty() || source_lookup.insert(label.clone(), i).is_some() {
                    return Err("Live source labels must be unique.".into());
                }
                let source_id = stable_id("src", &[&id, &i.to_string()]);
                index.session.sources.push(Source {
                    artifact_id: stable_id("artifact", &[&source_id, digest]),
                    source_id,
                    role: "other".into(),
                    sha256: digest.to_owned(),
                    status: "silent".into(),
                    provenance: json!({
                        "time_basis": "capture_session",
                        "source_label": label,
                        "start_frame": index.start,
                        "refinements": []
                    }),
                    ..Default::default()
                });
            }
            return Ok(());
        }
        if kind == "end" {
            let cutoff = number(&v, "cutoff_frame")?;
            if cutoff < max_frame || (number(&v, "speech_entries")?, number(&v, "gaps")?) != counts
            {
                return Err("Final count or cutoff differs from the index.".into());
            }
            index.session.complete = !v["incomplete"]
                .as_bool()
                .ok_or("Missing completion state.")?;
            for s in &mut index.session.sources {
                s.provenance["recording_overlaps_frames"] = json!(index.recordings);
                s.duration_ms = Some((cutoff - index.start).div_ceil(48));
                s.provenance["cutoff_frame"] = json!(cutoff);
                if !s.gaps.is_empty() || !index.session.complete {
                    s.status = "partial".into()
                }
            }
            sealed = true;
            return Ok(());
        }
        if kind == "recording" {
            let bound = range(&v, "start_frame", "end_frame")?;
            if bound.0 < index.start
                || index
                    .recordings
                    .insert(string(&v, "id")?.into(), bound)
                    .is_some()
            {
                return Err("Invalid recording overlap.".into());
            }
            max_frame = max_frame.max(bound.1);
            return Ok(());
        }
        let src = *source_lookup
            .get(string(&v, "source")?)
            .ok_or("Unknown source in live index.")?;
        let source = &mut index.session.sources[src];
        if kind == "timing" {
            let f = number(&v, "frame")?;
            max_frame = max_frame.max(f);
            source
                .gaps
                .push(json!({"kind":"timing","start_ms":f/48,"frame":f}));
            return Ok(());
        }
        let (a, b) = range(&v, "start_frame", "end_frame")?;
        if a < index.start {
            return Err("Live record precedes interval.".into());
        }
        max_frame = max_frame.max(b);
        match kind {
            "request" => {}
            "speaker_revision" => {}
            "gap" | "skipped" => {
                counts.1 += u64::from(kind == "gap");
                source.gaps.push(json!({
                    "kind": if kind=="gap" {"recognition"} else {"coverage"},
                    "event": kind,
                    "reason": v["reason"].as_str(),
                    "start_ms": a / 48,
                    "end_ms": b.div_ceil(48),
                    "start_frame": a,
                    "end_frame": b
                }));
            }
            "speech" => {
                let raw = string(&v, "text")?;
                counts.0 += u64::from(!raw.trim().is_empty());
                if raw.is_empty() {
                    return Ok(());
                }
                let offset = source.raw_text.len();
                source.raw_text.push_str(raw);
                source.status = "success".into();
                let sid = source.source_id.clone();
                let artifact = source.artifact_id.clone();
                let parts = v["segments"].as_array().filter(|s| !s.is_empty());
                let mut pos = 0;
                let mut last = a;
                for (i, part) in parts
                    .map_or(std::slice::from_ref(&v), Vec::as_slice)
                    .iter()
                    .enumerate()
                {
                    let (x, y) = range(part, "start_frame", "end_frame")?;
                    let text = string(part, "text")?;
                    if x < last || y > b || !raw[pos..].starts_with(text) {
                        return Err("Timed speech differs from its original chunk.".into());
                    }
                    last = y;
                    let key = stable_id("turn", &[&sid, string(&v, "id")?, &i.to_string()]);
                    if index.bounds.insert(key.clone(), (x, y)).is_some() {
                        return Err("Duplicate live turn.".into());
                    }
                    let ids = part["speaker"]
                        .as_str()
                        .filter(|s| parts.is_some() && !s.is_empty())
                        .map(|l| {
                            speaker(
                                &mut index.session,
                                &mut seen,
                                stable_id("spk", &[&sid, "live", l]),
                            )
                        })
                        .into_iter()
                        .collect::<Vec<_>>();
                    turn_indices.insert(key.clone(), index.session.turns.len());
                    index.session.turns.push(Turn {
                        turn_id: key,
                        source_id: sid.clone(),
                        artifact_id: artifact.clone(),
                        raw_text: text.into(),
                        attribution: if ids.is_empty() {
                            "unresolved"
                        } else {
                            "provisional"
                        }
                        .into(),
                        timing_method: if parts.is_some() {
                            "provider_segment"
                        } else {
                            "chunk"
                        }
                        .into(),
                        span_bytes: [offset + pos, offset + pos + text.len()],
                        start_ms: Some(x / 48),
                        end_ms: Some(y.div_ceil(48)),
                        speaker_ids: ids,
                        ..Default::default()
                    });
                    pos += text.len();
                }
                if pos != raw.len() {
                    return Err("Timed speech omits original text.".into());
                }
            }
            _ => return Err("Unknown live index record.".into()),
        }
        if let Some(assignments) = v["assignments"].as_array() {
            let sid = &index.session.sources[src].source_id;
            let sid = sid.clone();
            for row in assignments {
                let id = stable_id(
                    "turn",
                    &[
                        &sid,
                        string(row, "id")?,
                        &number(row, "ordinal")?.to_string(),
                    ],
                );
                if index.bounds.get(&id) != Some(&range(row, "start_frame", "end_frame")?) {
                    return Err("Speaker revision refers to a different speech span.".into());
                }
                let i = turn_indices[&id];
                if index.session.turns[i].timing_method == "chunk" {
                    return Err("Speaker revision requires timed speech.".into());
                }
                let epoch = number(row, "epoch")?.to_string();
                let ids = row["speaker"]
                    .as_str()
                    .map(|s| {
                        speaker(
                            &mut index.session,
                            &mut seen,
                            stable_id("spk", &[&sid, "local-live", &epoch, s]),
                        )
                    })
                    .into_iter()
                    .collect::<Vec<_>>();
                let t = &mut index.session.turns[i];
                t.attribution = if ids.is_empty() {
                    "unresolved"
                } else {
                    "provisional"
                }
                .into();
                t.speaker_ids = ids;
            }
        }
        Ok(())
    })?;
    if !sealed {
        return Err("Select a finalized live transcript index.".into());
    }
    super::verify_spans(&index.session)?;
    super::order_turns(&mut index.session);
    Ok(index)
}
pub(super) fn run(path: &Path, target: &Path, media: Option<&Media>) -> Result<Session> {
    if !path
        .file_name()
        .and_then(|f| f.to_str())
        .is_some_and(|l| l.ends_with(".segments.jsonl"))
    {
        return Err("Select the completed .segments.jsonl index.".into());
    }
    export::local_destination(target)?;
    let mut file = pinned(path)?;
    let digest = hash(&mut file)?;
    let mut index = parse(&file, &digest)?;
    let refinement = media
        .map(|choice| timeline::prepare(&index, choice))
        .transpose()?;
    let manifest = Manifest {
        session_id: index.session.session_id.clone(),
        representation: "live".into(),
        shared_clock: true,
        ..Default::default()
    };
    let data = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    match jobs::stage_and_publish_new(target, &data) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("Cannot publish live manifest: {e}")),
    }
    let existing = store::manifest(target)?;
    if existing.session_id != manifest.session_id || existing.representation != "live" {
        return Err("Manifest belongs to another session.".into());
    }
    let (context, naming_key) =
        super::name_context(target.parent().unwrap_or(Path::new(".")), &existing)?;
    index.session.naming_key = naming_key.clone();
    let baseline = store::update(target, &existing, |old| match old {
        Some(s) if s.analysis_id != digest => {
            Err("The original live index changed. Select its immutable version.".into())
        }
        Some(s) => Ok(s.clone()),
        None => Ok(index.session),
    })?;
    let Some(refinement) = refinement else {
        return Ok(baseline);
    };
    drop(baseline);
    store::update(target, &existing, |old| {
        let old = old.ok_or("Missing original live session.")?;
        let mut session = refinement.apply(old, &index.bounds)?;
        if session.revision != old.revision {
            super::naming::infer(&mut session, &context, existing.automatic_names)?;
            session.naming_key = naming_key;
        }
        Ok(session)
    })
}
