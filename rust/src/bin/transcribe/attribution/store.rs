use super::model::{Manifest, SCHEMA_MAJOR, Session, valid_id};
use crate::jobs;
use serde::de::DeserializeOwned;
use std::fs::{self, File};
use std::io::{BufReader, ErrorKind};
use std::path::{Path, PathBuf};

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = File::open(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    serde_json::from_reader(BufReader::new(file))
        .map_err(|e| format!("Invalid JSON in {}: {e}", path.display()))
}

pub(super) fn manifest(path: &Path) -> Result<Manifest, String> {
    let manifest: Manifest = read_json(path)?;
    if manifest.schema_major != SCHEMA_MAJOR || !valid_id(&manifest.session_id) {
        return Err("Invalid session identifier or unsupported manifest version".into());
    }
    Ok(manifest)
}

fn directory(path: &Path, id: &str) -> PathBuf {
    path.parent()
        .unwrap_or(Path::new("."))
        .join(".audio-capture-sessions")
        .join(id)
}

fn read_head(dir: &Path, id: &str) -> Result<Option<Session>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Cannot open session history: {e}")),
    };
    let mut highest = 0;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Cannot read session history: {e}"))?;
        let name = entry.file_name();
        if let Some(digits) = name.to_str().and_then(|s| s.strip_suffix(".json"))
            && digits.len() == 20
            && digits.bytes().all(|b| b.is_ascii_digit())
        {
            let number = digits
                .parse::<u64>()
                .map_err(|_| "Invalid session revision filename")?;
            if number == 0 {
                return Err("Invalid session revision zero".into());
            }
            highest = highest.max(number);
        }
    }
    if highest == 0 {
        return Ok(None);
    }
    let session: Session = read_json(&dir.join(format!("{highest:020}.json")))?;
    if session.schema_major != SCHEMA_MAJOR
        || session.session_id != id
        || session.revision != highest
    {
        return Err("Session history has an unexpected schema, identifier, or revision".into());
    }
    Ok(Some(session))
}

pub fn latest(path: &Path) -> Result<Session, String> {
    super::refresh_names(path)
}

/// A preview or export never creates a transcript revision. Require the
/// operator to refresh review first if previously permitted context changed.
pub fn for_export(path: &Path) -> Result<Session, String> {
    let manifest = manifest(path)?;
    let result = saved(path, &manifest)?;
    let (_, key) = super::name_context(path.parent().unwrap_or(Path::new(".")), &manifest)?;
    if result.naming_key != key {
        return Err("Naming evidence changed. Run review-session to refresh the transcript before exporting.".into());
    }
    Ok(result)
}

pub(super) fn saved(path: &Path, manifest: &Manifest) -> Result<Session, String> {
    read_head(&directory(path, &manifest.session_id), &manifest.session_id)?
        .ok_or_else(|| "This session has no saved transcript. Run attribute first.".into())
}

pub(super) fn update(
    path: &Path,
    manifest: &Manifest,
    build: impl FnOnce(Option<&Session>) -> Result<Session, String>,
) -> Result<Session, String> {
    let dir = directory(path, &manifest.session_id);
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create session history: {e}"))?;
    let _lock = jobs::lock(&dir.join("writer.lock"))?;
    let previous = read_head(&dir, &manifest.session_id)?;
    let session = build(previous.as_ref())?;
    if previous.as_ref() == Some(&session) {
        return Ok(session);
    }
    let expected = previous
        .as_ref()
        .map_or(0, |prev| prev.revision)
        .checked_add(1)
        .ok_or("Session revision limit reached")?;
    if session.revision != expected
        || session.session_id != manifest.session_id
        || session.schema_major != SCHEMA_MAJOR
    {
        return Err("New transcript does not match the session transaction".into());
    }
    let data =
        serde_json::to_vec(&session).map_err(|e| format!("Cannot serialize transcript: {e}"))?;
    jobs::stage_and_publish_new(&dir.join(format!("{expected:020}.json")), &data)
        .map_err(|e| format!("Cannot publish transcript revision: {e}"))?;
    Ok(session)
}
