pub(crate) mod adapter;
pub(crate) mod cli;
pub(crate) mod decisions;
pub(crate) mod export;
pub(crate) mod importer;
pub(crate) mod inputs;
mod live_import;
pub(crate) mod model;
pub(crate) mod naming;
pub(crate) mod store;

use crate::jobs;
use model::{
    Activity, Manifest, SCHEMA_MAJOR, Session, Source, SourceInput, Speaker, Turn, stable_id,
    valid_id,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

fn resolve(base: &Path, path: &Path) -> PathBuf {
    base.join(path)
}

pub(crate) fn select_sources<'a>(
    manifest: &'a Manifest,
    base: &Path,
) -> Result<Vec<&'a SourceInput>, String> {
    let mut ids = BTreeSet::new();
    for input in &manifest.sources {
        if !valid_id(&input.source_id) || !ids.insert(&input.source_id) {
            return Err("Each source needs a unique, safe identifier".into());
        }
        if !matches!(
            input.role.as_str(),
            "mic" | "program" | "system" | "mix" | "other"
        ) || input.audio.as_os_str().is_empty()
            || input.artifact.as_os_str().is_empty()
            || input.duration_ms == Some(0)
            || input
                .sha256
                .as_ref()
                .is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(format!(
                "Invalid role, path or SHA-256 for source {}",
                input.source_id
            ));
        }
    }
    let mixes = manifest.sources.iter().filter(|s| s.role == "mix").count();
    let has_stems = mixes < manifest.sources.len();
    let explicit = manifest.representation == "explicit";
    if !explicit && manifest.sources.iter().any(|s| s.selected.is_some()) {
        return Err("Use representation explicit to select individual sources".into());
    }
    let use_mix = match manifest.representation.as_str() {
        "auto" if mixes > 0 && has_stems && !manifest.complete_stems => {
            return Err(
                "Incomplete stems and a mix require an explicit representation choice".into(),
            );
        }
        "auto" => !has_stems,
        "mix" => true,
        "stems" | "explicit" => false,
        _ => return Err("Use representation auto, mix, stems, or explicit".into()),
    };
    let mut sources = Vec::new();
    for input in &manifest.sources {
        let include = if explicit {
            input
                .selected
                .ok_or("Select or exclude each source in explicit mode")?
        } else {
            (input.role == "mix") == use_mix
        };
        if include {
            sources.push(input);
        }
    }
    if sources.is_empty() {
        return Err("Select at least one canonical source".into());
    }
    let roles: BTreeSet<_> = sources.iter().map(|s| s.role.as_str()).collect();
    if (roles.contains("mix") && sources.len() != 1)
        || (roles.contains("system") && roles.contains("program") && manifest.shared_clock)
    {
        return Err(
            "Choose one representation for overlapping system, program, or mixed audio".into(),
        );
    }
    let mut recordings = BTreeSet::new();
    let mut artifacts = BTreeSet::new();
    let mut origins = BTreeSet::new();
    for s in &sources {
        let media = resolve(base, &s.audio);
        let artifact = resolve(base, &s.artifact);
        if !recordings.insert(media.canonicalize().unwrap_or(media))
            || !artifacts.insert(artifact.canonicalize().unwrap_or_else(|_| artifact.clone()))
        {
            return Err("A canonical input or recognition artifact was selected twice".into());
        }
        // Aggregate and single-part job receipts address the same full recording,
        // even if the user points at two different files in the manifest.
        if let Ok(Some(origin)) = inputs::artifact_source(&artifact)
            && !origins.insert(origin)
        {
            return Err(
                "An aggregate or recognition part duplicates another selected recording".into(),
            );
        }
    }
    Ok(sources)
}

fn name_context(base: &Path, manifest: &Manifest) -> Result<(Value, String), String> {
    let mut revisions = Vec::new();
    let mut config = manifest.naming_context.clone();
    if config.is_null() {
        config = json!({});
    }
    if let Some(entries) = config.get_mut("prior_context") {
        for entry in entries
            .as_array_mut()
            .ok_or("prior_context must be an array")?
        {
            let path = entry
                .get("session")
                .and_then(Value::as_str)
                .ok_or("A prior context needs its session manifest")?;
            if entry.get("snapshot").is_some() {
                return Err("Use a prior session manifest instead of an inline snapshot".into());
            }
            let target = resolve(base, Path::new(path));
            let prior = store::manifest(&target).and_then(|m| store::saved(&target, &m));
            let snapshot = match prior {
                Ok(s) if s.session_id != manifest.session_id => {
                    revisions.push(Some((s.session_id.clone(), s.revision)));
                    serde_json::to_value(s).map_err(|e| e.to_string())?
                }
                Ok(_) => return Err("A session cannot use itself as prior naming context".into()),
                Err(e) => {
                    revisions.push(None);
                    eprintln!("Prior naming context unavailable: {e}");
                    Value::Null
                }
            };
            entry["snapshot"] = snapshot;
        }
    }
    let key = serde_json::to_vec(&(
        1,
        manifest.automatic_names,
        &manifest.naming_context,
        revisions,
    ))
    .map_err(|e| e.to_string())?;
    Ok((config, jobs::digest(key.as_slice())?))
}

fn refresh_names(path: &Path) -> Result<Session, String> {
    let manifest = store::manifest(path)?;
    let current = store::saved(path, &manifest)?;
    let (context, key) = name_context(path.parent().unwrap_or(Path::new(".")), &manifest)?;
    if current.naming_key == key {
        return Ok(current);
    }
    drop(current);
    store::update(path, &manifest, |previous| {
        let mut session = previous
            .cloned()
            .ok_or("Run attribute before reviewing this session")?;
        if session.naming_key != key {
            session.revision = session
                .revision
                .checked_add(1)
                .ok_or("Session revision limit reached")?;
            naming::infer(&mut session, &context, manifest.automatic_names)?;
            session.naming_key = key;
        }
        Ok(session)
    })
}

// The backend's cluster labels are local to one invocation. Use the actual
// regions of support to preserve continuity across a label permutation.
fn speaker_ids(
    session: &str,
    source: &str,
    sha: &str,
    backend: &str,
    rec: &inputs::Recognition,
) -> Result<BTreeMap<String, String>, String> {
    let mut support: BTreeMap<&str, Vec<(u8, u64, u64)>> = BTreeMap::new();
    for interval in &rec.activity {
        support
            .entry(&interval.label)
            .or_default()
            .push((0, interval.start_ms, interval.end_ms));
    }
    for span in &rec.spans {
        for label in &span.labels {
            support
                .entry(label)
                .or_default()
                .push((1, span.bytes[0] as u64, span.bytes[1] as u64));
        }
    }
    let mut by_support: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (label, mut regions) in support {
        regions.sort_unstable();
        regions.dedup();
        let bytes = serde_json::to_vec(&regions).map_err(|e| e.to_string())?;
        by_support
            .entry(jobs::digest(bytes.as_slice())?)
            .or_default()
            .push(label);
    }
    let mut ids = BTreeMap::new();
    for (digest, labels) in by_support {
        let unique = labels.len() == 1;
        for label in labels {
            // Two indistinguishable concurrent clusters still require distinct IDs.
            let tie = if unique { "" } else { label };
            ids.insert(
                label.to_owned(),
                stable_id("sp", &[session, source, sha, backend, &digest, tie]),
            );
        }
    }
    Ok(ids)
}

fn overlap_ranges(activity: &[inputs::Interval]) -> Vec<(u64, u64)> {
    let mut events = Vec::with_capacity(activity.len().saturating_mul(2));
    for i in activity {
        events.push((i.start_ms, 1i32, i.label.as_str()));
        events.push((i.end_ms, -1i32, i.label.as_str()));
    }
    events.sort_unstable();
    let mut active = BTreeMap::new();
    let mut previous = 0;
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    for (time, delta, label) in events {
        if previous < time && active.len() > 1 {
            if let Some(range) = ranges.last_mut()
                && range.1 == previous
            {
                range.1 = time;
            } else {
                ranges.push((previous, time));
            }
        }
        let count = active.entry(label).or_insert(0);
        *count += delta;
        if *count == 0 {
            active.remove(label);
        }
        previous = time;
    }
    ranges
}

fn recognize(
    session: &mut Session,
    input: &SourceInput,
    base: &Path,
    config: Option<&adapter::AdapterConfig>,
    require_recognition: bool,
) -> Result<(), String> {
    let audio = resolve(base, &input.audio);
    let artifact = resolve(base, &input.artifact);
    let mut source = Source {
        source_id: input.source_id.clone(),
        role: input.role.clone(),
        sha256: session
            .source_bindings
            .get(&input.source_id)
            .cloned()
            .unwrap_or_default(),
        status: "failed".into(),
        duration_ms: input.duration_ms,
        provenance: json!({"audio":audio,"artifact":artifact}),
        ..Default::default()
    };
    // Keep the media pinned until the adapter and duration probe finish reading it.
    let pinned = match OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&audio)
    {
        Ok(file) => Some(file),
        Err(e) => {
            source
                .gaps
                .push(json!({"kind":"missing_source","message":e.to_string()}));
            None
        }
    };
    if let Some(file) = &pinned {
        let sha = jobs::digest(file)?;
        if input
            .sha256
            .as_ref()
            .is_some_and(|pin| !pin.eq_ignore_ascii_case(&sha))
            || session
                .source_bindings
                .get(&input.source_id)
                .is_some_and(|pin| pin != &sha)
        {
            return Err(format!(
                "Audio bytes changed for {}. Register a new source ID for the replacement.",
                input.source_id
            ));
        }
        if session.sources.iter().any(|other| other.sha256 == sha) {
            return Err(
                "Duplicate canonical audio bytes. Select one source for this recording.".into(),
            );
        }
        session
            .source_bindings
            .insert(input.source_id.clone(), sha.clone());
        source.sha256 = sha;
    } else if source.sha256.is_empty() {
        source.sha256 = input.sha256.clone().unwrap_or_default();
    }
    if source.duration_ms.is_none() && source.gaps.is_empty() {
        source.duration_ms =
            jobs::output_duration(&audio).map(|seconds| (seconds * 1000.0).ceil() as u64);
    }
    let (mut rec, recognition_failed) =
        match inputs::load(&artifact, &source.sha256, source.duration_ms) {
            Ok(rec) => (rec, false),
            Err(e) => {
                source.gaps.push(json!({"kind":"recognition","message":e}));
                (inputs::Recognition::default(), true)
            }
        };
    let mut backend = String::from("provider");
    source.provenance["asr_model"] = json!(rec.model);
    source.provenance["provider_profile"] = json!(rec.provider_profile);
    source.provenance["attribution_status"] = json!(if rec.activity.is_empty() {
        "unavailable"
    } else {
        "provider"
    });
    if let Some(config) = config
        && (!require_recognition || !recognition_failed)
    {
        let result = source
            .duration_ms
            .ok_or_else(|| "A verified duration is required for local diarization".to_owned())
            .and_then(|duration| adapter::run(config, &input.source_id, &audio, duration));
        match result {
            Ok(output) => {
                source.provenance["attribution_status"] = json!("adapter");
                backend = format!("{}:{}", output.backend, output.model);
                source.provenance["diarization"] =
                    json!({"backend":output.backend,"model":output.model});
                let activity = output
                    .intervals
                    .into_iter()
                    .map(|v| inputs::Interval {
                        label: v.speaker,
                        start_ms: v.start_ms,
                        end_ms: v.end_ms,
                    })
                    .collect();
                inputs::align(&mut rec, activity);
            }
            Err(e) => {
                source.provenance["attribution_status"] = json!("adapter_failed");
                source.provenance["adapter_error"] = json!(e);
            }
        }
    }
    source.status = if recognition_failed {
        "failed"
    } else if !source.gaps.is_empty() || !rec.issues.is_empty() {
        "partial"
    } else if rec.text.is_empty() && rec.activity.is_empty() {
        "silent"
    } else if !rec.text.is_empty() && rec.spans.iter().all(|s| s.start_ms.is_none()) {
        "untimed"
    } else {
        "success"
    }
    .into();

    for issue in &rec.issues {
        let bounds = rec
            .gaps
            .iter()
            .find(|(_, message)| message == issue)
            .map(|(bounds, _)| bounds);
        source.gaps.push(json!({"kind":"coverage","message":issue,"start_ms":bounds.map(|b|b[0]),"end_ms":bounds.map(|b|b[1])}));
    }
    source.artifact_id = stable_id(
        "artifact",
        &[&session.session_id, &source.source_id, &rec.text],
    );
    let speakers = speaker_ids(
        &session.session_id,
        &source.source_id,
        &source.sha256,
        &backend,
        &rec,
    )?;
    let overlap = overlap_ranges(&rec.activity);
    for speaker_id in speakers.values() {
        session.speakers.push(Speaker {
            speaker_id: speaker_id.clone(),
            name_status: "anonymous".into(),
            ..Default::default()
        });
    }
    let mut interval_ids = BTreeSet::new();
    for interval in rec.activity {
        let speaker_id = speakers[&interval.label].clone();
        let activity_id = stable_id(
            "act",
            &[
                &speaker_id,
                &interval.start_ms.to_string(),
                &interval.end_ms.to_string(),
            ],
        );
        if interval_ids.insert(activity_id.clone()) {
            session.activities.push(Activity {
                activity_id,
                source_id: source.source_id.clone(),
                speaker_id,
                backend: backend.clone(),
                start_ms: interval.start_ms,
                end_ms: interval.end_ms,
            });
        }
    }
    for span in rec.spans {
        let raw = rec
            .text
            .get(span.bytes[0]..span.bytes[1])
            .ok_or("Invalid UTF-8 recognition span")?;
        let mut speaker_ids: Vec<_> = span.labels.iter().map(|l| speakers[l].clone()).collect();
        speaker_ids.sort_unstable();
        speaker_ids.dedup();
        let attribution = if speaker_ids.len() == 1 {
            "resolved"
        } else if speaker_ids.len() > 1
            && span.start_ms.zip(span.end_ms).is_some_and(|(start, end)| {
                overlap
                    .get(overlap.partition_point(|r| r.1 <= start))
                    .is_some_and(|r| r.0 < end)
            })
        {
            "overlap"
        } else {
            "unresolved"
        };
        session.turns.push(Turn {
            turn_id: stable_id(
                "turn",
                &[
                    &source.artifact_id,
                    &span.bytes[0].to_string(),
                    &span.bytes[1].to_string(),
                ],
            ),
            source_id: source.source_id.clone(),
            artifact_id: source.artifact_id.clone(),
            raw_text: raw.into(),
            attribution: attribution.into(),
            timing_method: span.timing_method,
            span_bytes: span.bytes,
            start_ms: span.start_ms,
            end_ms: span.end_ms,
            speaker_ids,
            ..Default::default()
        });
    }
    source.raw_text = rec.text;
    session.sources.push(source);
    Ok(())
}

fn verify_spans(session: &Session) -> Result<(), String> {
    let mut partitions: BTreeMap<&str, Vec<&Turn>> = BTreeMap::new();
    for turn in &session.turns {
        partitions.entry(&turn.source_id).or_default().push(turn);
    }
    for source in &session.sources {
        let mut spans = partitions
            .remove(source.source_id.as_str())
            .unwrap_or_default();
        spans.sort_unstable_by_key(|t| t.span_bytes[0]);
        let mut offset = 0;
        for span in spans {
            if span.span_bytes[0] != offset
                || span.span_bytes[1] <= offset
                || source.raw_text.get(offset..span.span_bytes[1]) != Some(span.raw_text.as_str())
            {
                return Err(format!(
                    "Cannot reconstruct the original text for {}",
                    source.source_id
                ));
            }
            offset = span.span_bytes[1];
        }
        if offset != source.raw_text.len() {
            return Err(format!(
                "Incomplete recognition text for {}",
                source.source_id
            ));
        }
    }
    if !partitions.is_empty() {
        return Err("Transcript contains a turn without its source".into());
    }
    Ok(())
}

fn order_turns(session: &mut Session) {
    session.turns.sort_by(|a, b| {
        if session.shared_clock {
            let key = |t: &Turn| t.start_ms.map_or((true, 0), |s| (false, s));
            key(a)
                .cmp(&key(b))
                .then(a.source_id.cmp(&b.source_id))
                .then_with(|| {
                    if a.start_ms.is_none() {
                        a.span_bytes.cmp(&b.span_bytes)
                    } else {
                        a.turn_id.cmp(&b.turn_id)
                    }
                })
        } else {
            (&a.source_id, &a.span_bytes).cmp(&(&b.source_id, &b.span_bytes))
        }
    });
}

pub fn attribute(path: &Path) -> Result<Session, String> {
    attribute_with_eligibility(path, None)
}

/// A batch can exclude unapproved or changed recordings before local analysis.
/// Retain their source IDs and explain each gap in the same revision history.
pub(crate) fn attribute_with_eligibility(
    path: &Path,
    excluded: Option<&BTreeMap<String, String>>,
) -> Result<Session, String> {
    let manifest = store::manifest(path)?;
    let full_path = std::path::absolute(path).map_err(|e| e.to_string())?;
    let base = full_path
        .parent()
        .ok_or("Session manifest needs a directory")?;
    let sources = select_sources(&manifest, base)?;
    let mut adapter = manifest.adapter.clone();
    if let Some(config) = &mut adapter
        && config.program.components().count() > 1
    {
        config.program = resolve(base, &config.program);
    }
    let (context, naming_key) = name_context(base, &manifest)?;
    store::update(path, &manifest, |previous| {
        let mut result = Session {
            schema_major: SCHEMA_MAJOR,
            naming_key,
            session_id: manifest.session_id.clone(),
            revision: previous.map_or(1, |s| s.revision),
            shared_clock: manifest.shared_clock,
            source_bindings: previous
                .map(|s| s.source_bindings.clone())
                .unwrap_or_default(),
            alternates: manifest
                .sources
                .iter()
                .filter(|s| !sources.iter().any(|a| a.source_id == s.source_id))
                .map(|s| s.source_id.clone())
                .collect(),
            ..Default::default()
        };
        for &input in &sources {
            if let Some(reason) = excluded.and_then(|sources| sources.get(&input.source_id)) {
                result.sources.push(Source {
                    source_id: input.source_id.clone(),
                    role: input.role.clone(),
                    status: "failed".into(),
                    sha256: result
                        .source_bindings
                        .get(&input.source_id)
                        .cloned()
                        .unwrap_or_default(),
                    gaps: vec![json!({"kind":"batch_selection","message":reason})],
                    ..Default::default()
                });
                continue;
            }
            recognize(
                &mut result,
                input,
                base,
                adapter.as_ref(),
                excluded.is_some(),
            )?;
        }
        result.alternates.sort_unstable();
        result
            .sources
            .sort_unstable_by(|a, b| a.source_id.cmp(&b.source_id));
        result
            .speakers
            .sort_unstable_by(|a, b| a.speaker_id.cmp(&b.speaker_id));
        result.activities.sort_unstable_by(|a, b| {
            (&a.source_id, a.start_ms, a.end_ms, &a.speaker_id).cmp(&(
                &b.source_id,
                b.start_ms,
                b.end_ms,
                &b.speaker_id,
            ))
        });
        result.complete = result
            .sources
            .iter()
            .all(|s| matches!(s.status.as_str(), "success" | "silent" | "untimed"));
        order_turns(&mut result);
        verify_spans(&result)?;
        // Exclude display paths and revision counters from the analysis identity.
        let mut identity = Vec::new();
        serde_json::to_writer(
            &mut identity,
            &(
                &result.shared_clock,
                &result.alternates,
                &result.turns,
                &result.activities,
                result
                    .sources
                    .iter()
                    .map(|s| {
                        (
                            &s.source_id,
                            &s.role,
                            &s.status,
                            &s.artifact_id,
                            &s.sha256,
                            &s.duration_ms,
                            &s.gaps,
                            s.provenance.get("diarization"),
                            s.provenance.get("asr_model"),
                            s.provenance.get("provider_profile"),
                        )
                    })
                    .collect::<Vec<_>>(),
            ),
        )
        .map_err(|e| e.to_string())?;
        result.analysis_id = jobs::digest(identity.as_slice())?;
        if let Some(old) = previous
            && !old.decisions.is_empty()
        {
            if old.analysis_id != result.analysis_id {
                return Err("This session has reviewed changes and different recognition or diarization results. Keep its history and prepare a new session for the new analysis.".into());
            }
            result.turns = old.turns.clone();
            result.activities = old.activities.clone();
            result.speakers = old.speakers.clone();
            result.decisions = old.decisions.clone();
        }
        naming::infer(&mut result, &context, manifest.automatic_names)?;
        if let Some(old) = previous
            && result != *old
        {
            result.revision = old
                .revision
                .checked_add(1)
                .ok_or("Session revision limit reached")?;
            naming::infer(&mut result, &context, manifest.automatic_names)?;
        }
        Ok(result)
    })
}

pub fn review(path: &Path, expected_revision: u64, command: Value) -> Result<Session, String> {
    let manifest = store::manifest(path)?;
    let patch: decisions::ReviewPatch =
        serde_json::from_value(command).map_err(|e| format!("Invalid review patch: {e}"))?;
    let (context, naming_key) = name_context(path.parent().unwrap_or(Path::new(".")), &manifest)?;
    store::update(path, &manifest, |previous| {
        let previous = previous.ok_or("Run attribute before reviewing this session")?;
        if previous.naming_key != naming_key {
            return Err(
                "Naming evidence changed. Open review-session before applying the patch.".into(),
            );
        }
        if previous.revision != expected_revision || patch.expected_revision != expected_revision {
            return Err("Review revision is stale".into());
        }
        let mut session = decisions::apply(previous, &patch)?;
        session.naming_key = naming_key;
        session.revision = expected_revision
            .checked_add(1)
            .ok_or("Session revision limit reached")?;
        order_turns(&mut session);
        verify_spans(&session)?;
        naming::infer(&mut session, &context, manifest.automatic_names)?;
        Ok(session)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    struct Fixture {
        dir: PathBuf,
        path: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(model::new_id("attribution_test").unwrap());
            fs::create_dir(&dir).unwrap();
            let path = dir.join("call.json");
            fs::write(dir.join("one.fake"), b"synthetic fixture media bytes").unwrap();
            fs::write(dir.join("recognition.txt"), "A synthetic turn.\n").unwrap();
            Self { dir, path }
        }
        fn manifest(&self) -> Manifest {
            Manifest {
                session_id: "s_demo".into(),
                shared_clock: true,
                sources: vec![SourceInput {
                    source_id: "src_one".into(),
                    role: "program".into(),
                    audio: "one.fake".into(),
                    artifact: "recognition.txt".into(),
                    duration_ms: Some(2000),
                    ..Default::default()
                }],
                ..Default::default()
            }
        }
        fn write(&self, m: &Manifest) {
            fs::write(&self.path, serde_json::to_vec(m).unwrap()).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn canonical_selection_requires_a_coverage_choice() {
        let f = Fixture::new();
        let mut m = f.manifest();
        m.sources.push(SourceInput {
            source_id: "src_mix".into(),
            role: "mix".into(),
            audio: "mix.fake".into(),
            artifact: "mix.txt".into(),
            ..Default::default()
        });
        assert!(select_sources(&m, &f.dir).is_err());
        m.complete_stems = true;
        assert_eq!(select_sources(&m, &f.dir).unwrap()[0].source_id, "src_one");
        m.representation = "mix".into();
        assert_eq!(select_sources(&m, &f.dir).unwrap()[0].source_id, "src_mix");
        m.sources[1].role = "system".into();
        m.representation = "auto".into();
        assert!(select_sources(&m, &f.dir).is_err());
        m.representation = "explicit".into();
        m.sources[0].selected = Some(true);
        m.sources[1].selected = Some(false);
        assert_eq!(select_sources(&m, &f.dir).unwrap().len(), 1);
    }

    #[test]
    fn equivalent_paths_and_copied_media_cannot_enter_twice() {
        let f = Fixture::new();
        let mut m = f.manifest();
        let mut duplicate = m.sources[0].clone();
        duplicate.source_id = "src_other".into();
        duplicate.audio = f.dir.join("one.fake");
        duplicate.artifact = "other.txt".into();
        m.sources.push(duplicate);
        f.write(&m);
        assert!(attribute(&f.path).unwrap_err().contains("twice"));
        fs::copy(f.dir.join("one.fake"), f.dir.join("copy.fake")).unwrap();
        m.sources[1].audio = "copy.fake".into();
        f.write(&m);
        assert!(
            attribute(&f.path)
                .unwrap_err()
                .contains("Duplicate canonical audio")
        );
        assert!(store::latest(&f.path).is_err());
    }

    #[test]
    fn history_survives_renames_replays_and_stale_patches() {
        let f = Fixture::new();
        let mut m = f.manifest();
        f.write(&m);
        let first = attribute(&f.path).unwrap();
        assert_eq!(first.revision, 1);
        assert!(first.complete);
        assert_eq!(first.sources[0].status, "untimed");
        assert_eq!(attribute(&f.path).unwrap(), first);
        let turn = first.turns[0].turn_id.clone();
        let patch = json!({"session_id":m.session_id,"expected_revision":1,"operations":[{"op":"correct_text","turn_id":turn,"text":"Corrected."}]});
        let corrected = review(&f.path, 1, patch.clone()).unwrap();
        assert_eq!(corrected.revision, 2);
        assert!(review(&f.path, 1, patch).is_err());
        assert_eq!(store::latest(&f.path).unwrap(), corrected);
        assert_eq!(attribute(&f.path).unwrap(), corrected);
        fs::rename(f.dir.join("one.fake"), f.dir.join("renamed.fake")).unwrap();
        m.sources[0].audio = "renamed.fake".into();
        f.write(&m);
        let moved = attribute(&f.path).unwrap();
        assert_eq!(moved.revision, 3);
        assert_eq!(moved.turns, corrected.turns);
        fs::write(f.dir.join("renamed.fake"), b"replacement").unwrap();
        assert!(attribute(&f.path).unwrap_err().contains("bytes changed"));
        assert_eq!(store::latest(&f.path).unwrap(), moved);
        assert!(
            f.dir
                .join(".audio-capture-sessions/s_demo/00000000000000000001.json")
                .exists()
        );
    }

    #[test]
    fn batch_exclusions_preserve_source_ids_and_eligible_text() {
        let fixture = Fixture::new();
        let mut manifest = fixture.manifest();
        manifest.sources.push(SourceInput {
            source_id: "src_two".into(),
            role: "mic".into(),
            audio: "unapproved.fake".into(),
            artifact: "unapproved.txt".into(),
            ..Default::default()
        });
        fixture.write(&manifest);
        let excluded =
            BTreeMap::from([("src_two".into(), "Outside the completed selection".into())]);
        let session = attribute_with_eligibility(&fixture.path, Some(&excluded)).unwrap();
        assert!(!session.complete);
        assert_eq!(session.sources[0].raw_text, "A synthetic turn.\n");
        assert_eq!(session.sources[1].source_id, "src_two");
        assert_eq!(session.sources[1].gaps[0]["kind"], "batch_selection");
    }

    #[test]
    fn batch_speaker_analysis_requires_a_readable_recognition_artifact() {
        let fixture = Fixture::new();
        let mut manifest = fixture.manifest();
        manifest.sources[0].artifact = "missing.txt".into();
        manifest.adapter = Some(adapter::AdapterConfig {
            program: fixture.dir.join("adapter-must-not-start.exe"),
            args: vec![],
            timeout_seconds: 1,
        });
        fixture.write(&manifest);
        let session = attribute_with_eligibility(&fixture.path, Some(&BTreeMap::new())).unwrap();
        let source = &session.sources[0];
        assert_eq!(source.status, "failed");
        assert!(source.gaps.iter().any(|gap| gap["kind"] == "recognition"));
        assert!(source.provenance.get("adapter_error").is_none());
    }

    #[test]
    fn missing_audio_retains_selected_asr_and_failed_adapter_is_partial() {
        let f = Fixture::new();
        let mut m = f.manifest();
        m.sources[0].audio = "missing.fake".into();
        m.adapter = Some(adapter::AdapterConfig {
            program: f.dir.join("missing-adapter.exe"),
            args: vec![],
            timeout_seconds: 1,
        });
        f.write(&m);
        let doc = attribute(&f.path).unwrap();
        assert!(!doc.complete);
        let s = &doc.sources[0];
        assert_eq!(s.raw_text, "A synthetic turn.\n");
        assert_eq!(s.provenance["attribution_status"], "adapter_failed");
        assert!(s.gaps.iter().any(|g| g["kind"] == "missing_source"));
        assert_eq!(doc.turns[0].attribution, "unresolved");
    }

    #[test]
    fn aggregate_and_part_receipts_share_the_same_media_identity() {
        let f = Fixture::new();
        let mut m = f.manifest();
        let origin = "a".repeat(64);
        let file = f.dir.join("whole.json");
        fs::write(
            file,
            serde_json::to_vec(&json!({"source_sha256":origin,"parts":[]})).unwrap(),
        )
        .unwrap();
        m.sources[0].artifact = "whole.json".into();
        let mut part = m.sources[0].clone();
        part.source_id = "part".into();
        part.audio = "other.fake".into();
        part.artifact = "chunk.json".into();
        fs::write(
            f.dir.join("chunk.json"),
            serde_json::to_vec(&json!({"source_sha256":origin,"text":"example"})).unwrap(),
        )
        .unwrap();
        m.sources.push(part);
        assert!(
            select_sources(&m, &f.dir)
                .unwrap_err()
                .contains("duplicates")
        );
    }

    #[test]
    fn backend_label_permutation_keeps_only_evidence_supported_speaker_ids() {
        let f = Fixture::new();
        let mut m = f.manifest();
        m.sources[0].artifact = "words.json".into();
        f.write(&m);
        let write = |first: &str, second: &str, end: f64| {
            let rec = json!({"text":"Hello. Bye.","segments":[{"text":"Hello.","start":0.1,"end":end,"speaker":first},{"text":"Bye.","start":1.0,"end":1.4,"speaker":second}]});
            fs::write(f.dir.join("words.json"), serde_json::to_vec(&rec).unwrap()).unwrap();
        };
        write("A", "B", 0.8);
        let original = attribute(&f.path).unwrap();
        assert_eq!(original.speakers.len(), 2);
        write("B", "A", 0.8);
        let swapped = attribute(&f.path).unwrap();
        assert_eq!(original, swapped);
        write("B", "A", 0.45);
        let changed = attribute(&f.path).unwrap();
        assert_ne!(changed.turns[0].speaker_ids, original.turns[0].speaker_ids);
        assert_eq!(changed.turns[1].speaker_ids, original.turns[1].speaker_ids);
    }

    #[test]
    fn changing_name_policy_refreshes_review_without_running_an_adapter() {
        let f = Fixture::new();
        let mut m = f.manifest();
        m.sources[0].artifact = "words.json".into();
        m.automatic_names = true;
        fs::write(f.dir.join("words.json"),serde_json::to_vec(&json!({"text":"My name is Nia.","segments":[{"text":"My name is Nia.","start":0.1,"end":1.0,"speaker":"A"}]})).unwrap()).unwrap();
        f.write(&m);
        let original = attribute(&f.path).unwrap();
        assert_eq!(original.speakers[0].display_name.as_deref(), Some("Nia"));
        m.automatic_names = false;
        m.adapter = Some(adapter::AdapterConfig {
            program: "missing.exe".into(),
            args: vec![],
            timeout_seconds: 1,
        });
        f.write(&m);
        assert!(
            store::for_export(&f.path)
                .unwrap_err()
                .contains("review-session")
        );
        assert_eq!(store::saved(&f.path, &m).unwrap().revision, 1);
        let updated = store::latest(&f.path).unwrap();
        assert_eq!(updated.revision, 2);
        assert!(updated.speakers[0].display_name.is_none());
        assert_eq!(updated.sources, original.sources);
        assert_eq!(store::latest(&f.path).unwrap(), updated);
    }

    #[test]
    fn a_corrected_prior_introduction_invalidates_export_and_refreshes_local_review() {
        let f = Fixture::new();
        let mut m = f.manifest();
        let prior_path = f.dir.join("prior.json");
        let write_record = |file: &str, text: &str| {
            fs::write(f.dir.join(file),serde_json::to_vec(&json!({"text":text,"segments":[{"text":text,"start":0.1,"end":1.0,"speaker":"A"}]})).unwrap()).unwrap()
        };
        write_record("earlier-words.json", "My name is Nia.");
        write_record("current-words.json", "Hello.");
        m.sources[0].artifact = "earlier-words.json".into();
        m.session_id = "earlier".into();
        fs::write(&prior_path, serde_json::to_vec(&m).unwrap()).unwrap();
        let earlier = attribute(&prior_path).unwrap();
        m.session_id = "current".into();
        m.automatic_names = true;
        m.sources[0].artifact = "current-words.json".into();
        f.write(&m);
        let current = attribute(&f.path).unwrap();
        m.naming_context = json!({"prior_context":[{
            "session":"prior.json","prior_speaker_id":earlier.speakers[0].speaker_id,
            "current_speaker_id":current.speakers[0].speaker_id,"mapping_id":"reviewed_link","mapping_revision":1
        }]});
        f.write(&m);
        let named = store::latest(&f.path).unwrap();
        assert_eq!(named.speakers[0].display_name.as_deref(), Some("Nia"));
        let patch = json!({"session_id":"earlier","expected_revision":1,"operations":[{"op":"correct_text","turn_id":earlier.turns[0].turn_id,"text":"The audio was unclear."}]});
        review(&prior_path, 1, patch).unwrap();
        assert!(store::for_export(&f.path).is_err());
        let corrected = store::latest(&f.path).unwrap();
        assert!(corrected.speakers[0].display_name.is_none());
        assert!(corrected.revision > named.revision);
        assert_eq!(store::for_export(&f.path).unwrap(), corrected);
    }

    #[test]
    fn invalid_ids_cannot_select_a_store_outside_the_manifest() {
        let f = Fixture::new();
        let mut m = f.manifest();
        for id in [
            "../out",
            "C:alternate",
            "NUL",
            "lpt1",
            "out.json",
            "a/b",
            "",
        ] {
            m.session_id = id.into();
            f.write(&m);
            assert!(attribute(&f.path).is_err());
            assert!(store::latest(&f.path).is_err());
        }
    }

    #[test]
    fn disjoint_turn_boundaries_are_distinct_from_concurrent_speech() {
        let region = |label: &str, start_ms, end_ms| inputs::Interval {
            label: label.to_owned(),
            start_ms,
            end_ms,
        };
        assert!(overlap_ranges(&[region("a", 0, 500), region("b", 500, 900)]).is_empty());
        assert_eq!(
            overlap_ranges(&[
                region("a", 0, 500),
                region("a", 100, 600),
                region("b", 450, 700)
            ]),
            [(450, 600)]
        );
    }
}
