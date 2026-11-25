use super::storage::{create_file, open_owned, read_json, rename, unlink_owned, write_json};
use super::{CHECKPOINT, Interval, JOURNAL, LiveCache, META, PARTIAL, Result};
use crate::{attribution::model::new_id, jobs::digest};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

const INTENT: &str = "publication.json";
const RECEIPT: &str = "published.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Publication {
    interval: String,
    directory: PathBuf,
    number: u32,
    text_sha256: String,
    journal_sha256: String,
}

impl Publication {
    fn stem(&self) -> String {
        if self.number == 0 {
            format!("Transcript-{}", self.interval)
        } else {
            format!("Transcript-{}-{}", self.interval, self.number)
        }
    }
    fn text(&self) -> PathBuf {
        self.directory.join(format!("{}.txt", self.stem()))
    }
    fn index(&self) -> PathBuf {
        self.directory
            .join(format!("{}.segments.jsonl", self.stem()))
    }

    fn verify_manifest(&self, meta: &Interval) -> Result<()> {
        if self.interval != meta.interval
            || self.directory != meta.output
            || self.number > 10000
            || [&self.text_sha256, &self.journal_sha256]
                .iter()
                .any(|h| h.len() != 64 || !h.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err("Invalid live publication receipt".into());
        }
        Ok(())
    }
    fn verify_files(&self) -> Result<()> {
        for (file, expected) in [
            (self.text(), &self.text_sha256),
            (self.index(), &self.journal_sha256),
        ] {
            if digest(open_owned(&file, false)?)? != *expected {
                return Err(format!(
                    "Published file differs from its receipt: {}",
                    file.display()
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod stage_tests {
    use super::*;

    #[test]
    fn partial_destination_copy_reuses_only_its_original_file() {
        let dir = std::env::temp_dir().join(new_id("live-stage-test").unwrap());
        fs::create_dir(&dir).unwrap();
        let final_path = dir.join("Transcript-test.txt");
        let (path, mut first) = resume_stage(&dir, &final_path, "stage-text.json").unwrap();
        first.write_all(b"interrupted text").unwrap();
        drop(first);
        let (same, file) = resume_stage(&dir, &final_path, "stage-text.json").unwrap();
        assert_eq!(same, path);
        assert_eq!(file.metadata().unwrap().len(), 0);
        drop(file);
        fs::rename(&path, dir.join("original")).unwrap();
        fs::write(&path, b"unrelated replacement").unwrap();
        assert!(resume_stage(&dir, &final_path, "stage-text.json").is_err());
        let saved: Stage = read_json(&dir.join("stage-text.json")).unwrap();
        super::super::storage::unlink_matching(&path, Some(saved.file_id)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"unrelated replacement");
        fs::remove_dir_all(dir).unwrap();
    }
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

// Persist the NTFS file identity before copying a single transcript byte. A
// crash can leave staged bytes on the destination volume. Resume only that
// exact file; a replacement, symlink or hard link must survive untouched.
#[derive(Serialize, Deserialize)]
struct Stage {
    token: String,
    file_id: (u32, u64),
}

impl Stage {
    fn path_for(&self, destination: &Path) -> Result<PathBuf> {
        if !self
            .token
            .strip_prefix("stage_")
            .is_some_and(|s| s.len() == 32 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            return Err("Invalid live staging receipt".into());
        }
        Ok(destination.with_extension(format!("{}.pending", self.token)))
    }
}

fn resume_stage(dir: &Path, destination: &Path, record: &str) -> Result<(PathBuf, fs::File)> {
    if exists(&dir.join(record))? {
        let stage: Stage = read_json(&dir.join(record))?;
        let path = stage.path_for(destination)?;
        if exists(&path)? {
            let file = open_owned(&path, true)?;
            if super::storage::identity(&file)? != stage.file_id {
                return Err("Live staging filename now identifies another file".into());
            }
            file.set_len(0).map_err(|e| e.to_string())?;
            return Ok((path, file));
        }
    }
    let token = new_id("stage")?;
    let path = destination.with_extension(format!("{token}.pending"));
    let file = create_file(&path)?;
    write_json(
        dir,
        record,
        &Stage {
            token,
            file_id: super::storage::identity(&file)?,
        },
    )?;
    Ok((path, file))
}

fn copy_new(
    dir: &Path,
    record: &str,
    source: &Path,
    destination: &Path,
    expected: &str,
) -> Result<()> {
    if exists(destination)? {
        if digest(open_owned(destination, false)?)? == expected {
            return Ok(());
        }
        return Err(format!(
            "A different file owns {}. Select another destination.",
            destination.display()
        ));
    }
    let (stage, mut output) = resume_stage(dir, destination, record)?;
    let mut input = open_owned(source, false)?;
    io::copy(&mut input, &mut output)
        .and_then(|_| output.flush())
        .and_then(|_| output.sync_all())
        .map_err(|e| e.to_string())?;
    // Retain the identified stage on failure. A retry or an explicit discard
    // can reuse or remove it without touching an unrelated replacement.
    drop(output);
    rename(&stage, destination, false)
        .map_err(|e| format!("Cannot publish {}: {e}", destination.display()))
}

impl LiveCache {
    pub(super) fn published(dir: &Path) -> Result<bool> {
        exists(&dir.join(RECEIPT))
    }

    fn owned_receipt(&self, file: &str) -> Result<Publication> {
        let p: Publication = read_json(&self.owner.path.join(file))?;
        p.verify_manifest(&self.meta)?;
        Ok(p)
    }

    pub(super) fn clear_stages(&self) -> Result<()> {
        if !exists(&self.owner.path.join(INTENT))? {
            return Ok(());
        }
        let p = self.owned_receipt(INTENT)?;
        for (record, destination) in [
            ("stage-index.json", p.index()),
            ("stage-text.json", p.text()),
        ] {
            let metadata = self.owner.path.join(record);
            if !exists(&metadata)? {
                continue;
            }
            let stage: Stage = read_json(&metadata)?;
            let path = stage.path_for(&destination)?;
            // The previously selected destination may be disconnected. Never
            // let cleanup of its staging copy block recovery to another disk.
            let _ = super::storage::unlink_matching(&path, Some(stage.file_id));
            unlink_owned(&metadata)?;
        }
        Ok(())
    }

    pub(super) fn redirect_publication(&mut self, output: &Path) -> Result<()> {
        let output = std::path::absolute(output).map_err(|e| e.to_string())?;
        if output == self.meta.output {
            return Ok(());
        }
        if Self::published(&self.owner.path)? {
            return Err("The transcript already has a verified publication".into());
        }
        // A previous attempt may have safely published one of the two files.
        // Never alter an output in that directory when the user changes it.
        self.clear_stages()?;
        unlink_owned(&self.owner.path.join(INTENT))?;
        self.meta.output = output;
        write_json(&self.owner.path, META, &self.meta)
    }

    fn release_recovery_files(&self) -> Result<()> {
        self.clear_stages()?;
        for name in [
            PARTIAL,
            JOURNAL,
            CHECKPOINT,
            INTENT,
            "stage-index.json",
            "stage-text.json",
            "state.pending",
        ] {
            unlink_owned(&self.owner.path.join(name))?;
        }
        Ok(())
    }

    pub(super) fn publish(&mut self) -> Result<PathBuf> {
        if self.poisoned || self.lock.is_none() || !self.state.sealed {
            return Err("Only a sealed live transcript can be published".into());
        }
        if Self::published(&self.owner.path)? {
            let p = self.owned_receipt(RECEIPT)?;
            p.verify_files()?;
            self.release_recovery_files()?;
            return Ok(p.text());
        }

        let p = if exists(&self.owner.path.join(INTENT))? {
            self.owned_receipt(INTENT)?
        } else {
            fs::create_dir_all(&self.meta.output)
                .map_err(|e| format!("Cannot open publication directory: {e}"))?;
            let mut p = Publication {
                interval: self.meta.interval.clone(),
                directory: self.meta.output.clone(),
                number: 0,
                text_sha256: digest(open_owned(&self.partial_path(), false)?)?,
                journal_sha256: digest(open_owned(&self.owner.path.join(JOURNAL), false)?)?,
            };
            while exists(&p.text())? || exists(&p.index())? {
                p.number += 1;
                if p.number > 10000 {
                    return Err("No available live transcript filename".into());
                }
            }
            write_json(&self.owner.path, INTENT, &p)?;
            p
        };
        copy_new(
            &self.owner.path,
            "stage-index.json",
            &self.owner.path.join(JOURNAL),
            &p.index(),
            &p.journal_sha256,
        )?;
        // The TXT is the visible completion marker. Its index is present first.
        copy_new(
            &self.owner.path,
            "stage-text.json",
            &self.partial_path(),
            &p.text(),
            &p.text_sha256,
        )?;
        p.verify_files()?;
        write_json(&self.owner.path, RECEIPT, &p)?;
        self.release_recovery_files()?;
        Ok(p.text())
    }
}
