use super::{Failure, Result, invalid, storage};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(super) const PROFILE: &str = "local-whisper";
pub(super) const TRANSCRIBE: &str = "batch.transcribe";
pub(super) const STATUS: &str = "batch.status";
pub(super) const CANCEL: &str = "batch.cancel";
pub(super) const FETCH: &str = "transcript.fetch";
pub(super) const IMPORT: &str = "model.import";
const OPERATIONS: &[&str] = &[TRANSCRIBE, STATUS, CANCEL, FETCH, IMPORT];

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(super) struct Policy {
    pub input_roots: Vec<PathBuf>,
    pub output_roots: Vec<PathBuf>,
    pub operations: Vec<String>,
    pub profiles: Vec<String>,
    pub allow_headless_approval: bool,
}

impl Policy {
    pub fn validate(mut self) -> Result<Self> {
        if self.operations.is_empty()
            || self
                .operations
                .iter()
                .any(|s| !OPERATIONS.contains(&s.as_str()))
        {
            return Err(invalid("Select explicit supported operations."));
        }
        if self.profiles.as_slice() != [PROFILE] {
            return Err(invalid("Choose the local-whisper profile for this grant."));
        }
        if self.input_roots.is_empty()
            || self.input_roots.len() > 64
            || self.output_roots.len() > 64
            || self.operations.len() > OPERATIONS.len()
        {
            return Err(invalid(
                "Supply one to 64 local input roots and at most 64 output roots.",
            ));
        }
        if self.operations.iter().any(|s| s == TRANSCRIBE) && self.output_roots.is_empty() {
            return Err(invalid("Select an existing transcript output directory."));
        }
        for path in &mut self.input_roots {
            *path = storage::local_path(path)
                .map_err(|_| invalid("Choose a readable, existing local input root."))?;
        }
        for path in &mut self.output_roots {
            *path = storage::hold_dir(path)
                .map_err(|_| invalid("Choose an existing local output directory."))?
                .1;
        }
        Ok(self)
    }

    pub fn allows(&self, operation: &str) -> Result<()> {
        if self.operations.iter().any(|s| s == operation) {
            Ok(())
        } else {
            Err(Failure::denied())
        }
    }

    // Reject an unapproved path before opening it. An out-of-scope caller cannot
    // distinguish a missing file from a readable file. Recheck by handle after opening.
    pub fn requested_input(&self, path: &Path) -> Result<()> {
        let path = storage::lexical_path(path).ok_or_else(Failure::denied)?;
        self.input(&path)
    }

    pub fn requested_output(&self, path: &Path) -> Result<()> {
        let path = storage::lexical_path(path).ok_or_else(Failure::denied)?;
        self.output(&path)
    }

    pub fn input(&self, path: &Path) -> Result<()> {
        self.allowed_path(path, &self.input_roots)
    }

    pub fn output(&self, path: &Path) -> Result<()> {
        self.allowed_path(path, &self.output_roots)
    }

    fn allowed_path(&self, path: &Path, roots: &[PathBuf]) -> Result<()> {
        if roots.iter().any(|root| storage::is_within(path, root)) {
            Ok(())
        } else {
            Err(Failure::denied())
        }
    }
}

pub(super) fn grant(root: &Path, reference: &str) -> Result<Policy> {
    if !storage::valid_id(reference, "grant") {
        return Err(Failure::denied());
    }
    storage::read(&root.join("grants").join(reference)).map_err(|_| Failure::denied())
}
