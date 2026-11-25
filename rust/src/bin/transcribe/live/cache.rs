//! Per-interval, crash-recoverable live transcripts. The journal is the source
//! of truth. Each checkpoint covers only fully synchronized journal and TXT bytes.
mod alignment;
mod journal;
mod publish;
mod storage;
#[cfg(test)]
mod tests;

use crate::jobs;
pub(crate) use journal::TimedSpeech;
use journal::{Checkpoint, Event, Row};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use storage::{OwnedDir, local_root, owned_file, read_json, write_json};

type Result<T> = std::result::Result<T, String>;
const META: &str = "interval.json";
const CHECKPOINT: &str = "checkpoint.json";
const JOURNAL: &str = "segments.jsonl";
const PARTIAL: &str = "transcript.partial.txt";
const LOCK: &str = "writer.lock";
const SCHEMA: u32 = 1;
const HELP: &str = "Usage: transcribe recover-live list\n       transcribe recover-live publish INTERVAL [--output-dir DIR]\n       transcribe recover-live discard INTERVAL --confirm";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Interval {
    schema: u32,
    session: String,
    interval: String,
    start: u64,
    #[serde(default)]
    sources: Vec<String>,
    output: PathBuf,
}

fn safe_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || v == b'-' || v == b'_')
    {
        return Err("Invalid live interval or session ID".into());
    }
    Ok(())
}

pub(crate) struct LiveCache {
    owner: OwnedDir,
    lock: Option<File>,
    meta: Interval,
    state: Checkpoint,
    alignment: alignment::Alignment,
    preview: Option<String>,
    // After a partial write, only recovery may use the previous durable checkpoint.
    poisoned: bool,
}

impl LiveCache {
    pub(crate) fn create_with_sources(
        session: &str,
        interval: &str,
        output: &Path,
        start: u64,
        sources: &[String],
    ) -> Result<Self> {
        Self::create_in(&local_root()?, session, interval, output, start, sources)
    }

    fn create_in(
        root: &Path,
        session: &str,
        interval: &str,
        output: &Path,
        start: u64,
        sources: &[String],
    ) -> Result<Self> {
        safe_id(interval)?;
        safe_id(session)?;
        if sources.len() > 128 || sources.iter().any(|s| s.is_empty() || s.len() > 1024) {
            return Err("Invalid live transcript source list".into());
        }
        let output = std::path::absolute(output).map_err(|e| e.to_string())?;
        let owner = OwnedDir::create(root, interval)?;
        let lock = jobs::lock(&owner.path.join(LOCK))?;
        let meta = Interval {
            schema: SCHEMA,
            session: session.into(),
            interval: interval.into(),
            sources: sources.to_owned(),
            start,
            output,
        };
        storage::create_file(&owner.path.join(JOURNAL))?;
        storage::create_file(&owner.path.join(PARTIAL))?;
        let state = Checkpoint::default();
        write_json(&owner.path, CHECKPOINT, &state)?;
        write_json(&owner.path, META, &meta)?;
        let mut cache = Self {
            owner,
            lock: Some(lock),
            meta,
            state,
            poisoned: false,
            alignment: alignment::Alignment::default(),
            preview: None,
        };
        cache.append(Event::Begin {
            schema: SCHEMA,
            session: session.into(),
            interval: interval.into(),
            start_frame: start,
            sources: sources.to_owned(),
        })?;
        Ok(cache)
    }

    fn open(root: &Path, interval: &str) -> Result<Self> {
        Self::open_cache(root, interval, true)
    }

    fn open_cache(root: &Path, interval: &str, replay: bool) -> Result<Self> {
        safe_id(interval)?;
        let owner = OwnedDir::existing(root, interval)?;
        let meta: Interval = read_json(&owner.path.join(META))?;
        if meta.schema != SCHEMA
            || meta.interval != interval
            || safe_id(&meta.session).is_err()
            || !meta.output.is_absolute()
        {
            return Err("Invalid live interval manifest".into());
        }
        owned_file(&owner.path.join(LOCK))?;
        let lock = jobs::lock(&owner.path.join(LOCK))?;
        let already_published = Self::published(&owner.path)?;
        let state = if already_published || !replay {
            Checkpoint::published()
        } else {
            read_json(&owner.path.join(CHECKPOINT))?
        };
        let mut cache = Self {
            owner,
            lock: Some(lock),
            meta,
            state,
            poisoned: false,
            alignment: alignment::Alignment::default(),
            preview: None,
        };
        if !already_published && replay {
            cache.replay_tail()?;
        }
        Ok(cache)
    }

    pub(crate) fn partial_path(&self) -> PathBuf {
        self.owner.path.join(PARTIAL)
    }
    pub(crate) fn last_preview(&self) -> Option<&str> {
        self.preview.as_deref()
    }
    #[cfg(test)]
    fn dir(&self) -> &Path {
        &self.owner.path
    }

    pub(crate) fn request(&mut self, id: &str, source: &str, start: u64, end: u64) -> Result<()> {
        self.append(Event::Request(Row::new(id, source, start, end)?))
    }
    pub(crate) fn speech(
        &mut self,
        id: &str,
        source: &str,
        start: u64,
        end: u64,
        speaker: Option<&str>,
        text: &str,
    ) -> Result<()> {
        self.append(Event::Speech {
            row: Row::new(id, source, start, end)?,
            speaker: speaker.map(str::to_owned),
            text: text.into(),
            segments: Vec::new(),
            assignments: Vec::new(),
        })
    }
    pub(crate) fn speech_timed(
        &mut self,
        id: &str,
        source: &str,
        start: u64,
        end: u64,
        text: &str,
        mut segments: Vec<TimedSpeech>,
    ) -> Result<()> {
        let mut next = start;
        let mut remaining = text;
        let valid = segments.iter().all(|s| {
            let Some(rest) = remaining.strip_prefix(&s.text) else {
                return false;
            };
            if s.start_frame < next || s.end_frame < s.start_frame || s.end_frame > end {
                return false;
            }
            remaining = rest;
            next = s.end_frame;
            true
        }) && remaining.is_empty();
        if !valid {
            segments.clear();
        }
        self.append(Event::Speech {
            row: Row::new(id, source, start, end)?,
            speaker: None,
            text: text.into(),
            segments,
            assignments: Vec::new(),
        })
    }
    pub(crate) fn gap(
        &mut self,
        id: &str,
        source: &str,
        start: u64,
        end: u64,
        reason: &str,
    ) -> Result<()> {
        self.append(Event::Gap {
            row: Row::new(id, source, start, end)?,
            reason: reason.into(),
        })
    }
    pub(crate) fn skipped(&mut self, id: &str, source: &str, start: u64, end: u64) -> Result<()> {
        self.append(Event::Skipped(Row::new(id, source, start, end)?))
    }
    pub(crate) fn recording(&mut self, id: &str, start: u64, end: u64) -> Result<()> {
        safe_id(id)?;
        self.append(Event::Recording {
            id: id.into(),
            start_frame: start,
            end_frame: end,
        })
    }
    pub(crate) fn timing(&mut self, source: &str, frame: u64) -> Result<()> {
        self.append(Event::Timing {
            source: source.into(),
            frame,
        })
    }

    pub(crate) fn speaker_revision(
        &mut self,
        source: &str,
        start: u64,
        end: u64,
        revision: u64,
        update: &impl Serialize,
    ) -> Result<()> {
        let update = serde_json::to_value(update).map_err(|e| e.to_string())?;
        self.append(Event::SpeakerRevision {
            source: source.into(),
            start_frame: start,
            end_frame: end,
            revision,
            update,
            assignments: Vec::new(),
        })
    }

    pub(crate) fn finish(&mut self, cutoff: u64, incomplete: bool) -> Result<PathBuf> {
        if !self.state.sealed {
            for row in self.state.pending.values().cloned().collect::<Vec<_>>() {
                self.append(Event::Gap {
                    row,
                    reason: "Unknown remote outcome; no automatic retry".into(),
                })?;
            }
            self.append(Event::End {
                cutoff_frame: cutoff,
                incomplete: incomplete || self.state.gaps > 0,
                speech_entries: self.state.speech,
                gaps: self.state.gaps,
            })?;
        }
        self.publish()
    }

    fn discard_owned(&mut self) -> Result<()> {
        // Never recurse, even for a directory with a verified manifest. Files
        // that we did not create must survive cleanup.
        self.clear_stages()?;
        for name in [
            PARTIAL,
            JOURNAL,
            CHECKPOINT,
            "publication.json",
            "published.json",
            "stage-text.json",
            "stage-index.json",
            "state.pending",
            META,
        ] {
            storage::unlink_owned(&self.owner.path.join(name))?;
        }
        self.lock.take();
        storage::unlink_owned(&self.owner.path.join(LOCK))?;
        match fs::remove_dir(&self.owner.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

pub(crate) fn recover_cli(args: impl Iterator<Item = OsString>) -> Result<()> {
    let mut args = args;
    let action = args.next().unwrap_or_default();
    if action == "--help" || action == "-h" {
        println!("{HELP}");
        return Ok(());
    }
    let root = local_root()?;
    if action == "list" && args.next().is_none() {
        for entry in fs::read_dir(&root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let Some(interval) = name.to_str().and_then(|s| s.strip_prefix("interval-")) else {
                continue;
            };
            if safe_id(interval).is_err() || OwnedDir::existing(&root, interval).is_err() {
                continue;
            }
            if let Ok(meta) = read_json::<Interval>(&entry.path().join(META))
                && meta.schema == SCHEMA
                && meta.interval == interval
            {
                // Active intervals keep the exclusive writer handle.
                let lock_path = entry.path().join(LOCK);
                if owned_file(&lock_path).is_ok()
                    && let Ok(_lock) = jobs::lock(&lock_path)
                {
                    if !LiveCache::published(&entry.path())? {
                        println!("{interval}\t{}", entry.path().join(PARTIAL).display());
                    }
                }
            }
        }
        return Ok(());
    }
    let id = args.next().and_then(|s| s.into_string().ok()).ok_or(HELP)?;
    match action.to_str() {
        Some("publish") => {
            let output = match args.next() {
                None => None,
                Some(v) if v == "--output-dir" => Some(PathBuf::from(args.next().ok_or(HELP)?)),
                _ => return Err(HELP.into()),
            };
            if args.next().is_some() {
                return Err(HELP.into());
            }
            let mut cache = LiveCache::open(&root, &id)?;
            if let Some(path) = output {
                cache.redirect_publication(&path)?;
            }
            let path = cache.finish(cache.state.last_frame.max(cache.meta.start), true)?;
            println!("{}", path.display());
            Ok(())
        }
        Some("discard")
            if args.next().as_deref() == Some(std::ffi::OsStr::new("--confirm"))
                && args.next().is_none() =>
        {
            let mut cache = LiveCache::open_cache(&root, &id, false)?;
            cache.discard_owned()
        }
        _ => Err(HELP.into()),
    }
}
