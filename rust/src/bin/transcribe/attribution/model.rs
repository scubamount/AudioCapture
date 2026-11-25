use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};

use super::adapter::AdapterConfig;

pub const SCHEMA_MAJOR: u32 = 1;

fn default_representation() -> String {
    "auto".to_owned()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_major: u32,
    pub session_id: String,
    #[serde(default)]
    pub shared_clock: bool,
    #[serde(default = "default_representation")]
    pub representation: String,
    #[serde(default)]
    pub complete_stems: bool,
    pub sources: Vec<SourceInput>,
    #[serde(default)]
    pub adapter: Option<AdapterConfig>,
    #[serde(default)]
    pub automatic_names: bool,
    #[serde(default, alias = "naming")]
    pub naming_context: Value,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            schema_major: SCHEMA_MAJOR,
            session_id: String::new(),
            shared_clock: false,
            representation: default_representation(),
            complete_stems: false,
            sources: vec![],
            adapter: None,
            automatic_names: false,
            naming_context: Value::Null,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceInput {
    pub source_id: String,
    pub role: String,
    pub audio: PathBuf,
    pub artifact: PathBuf,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub selected: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub schema_major: u32,
    pub session_id: String,
    pub revision: u64,
    pub shared_clock: bool,
    pub complete: bool,
    pub sources: Vec<Source>,
    pub turns: Vec<Turn>,
    pub activities: Vec<Activity>,
    pub speakers: Vec<Speaker>,
    pub decisions: Vec<Value>,
    pub alternates: Vec<String>,
    #[serde(default)]
    pub source_bindings: BTreeMap<String, String>,
    #[serde(default)]
    pub analysis_id: String,
    #[serde(default)]
    pub naming_key: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub source_id: String,
    pub role: String,
    pub status: String,
    pub raw_text: String,
    pub artifact_id: String,
    pub sha256: String,
    pub duration_ms: Option<u64>,
    pub provenance: Value,
    pub gaps: Vec<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub turn_id: String,
    pub source_id: String,
    pub artifact_id: String,
    pub raw_text: String,
    pub attribution: String,
    pub timing_method: String,
    pub span_bytes: [usize; 2],
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub speaker_ids: Vec<String>,
    pub reviewed_text: Option<String>,
    pub predecessors: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    pub speaker_id: String,
    pub display_name: Option<String>,
    pub person_id: Option<String>,
    pub name_status: String,
    pub evidence: Vec<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    pub activity_id: String,
    pub source_id: String,
    pub speaker_id: String,
    pub backend: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

pub fn valid_id(id: &str) -> bool {
    if id.is_empty()
        || id.len() > 96
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return false;
    }
    let name = id.to_ascii_uppercase();
    !matches!(name.as_str(), "NUL" | "CON" | "PRN" | "AUX")
        && !(name.len() == 4
            && (name.starts_with("COM") || name.starts_with("LPT"))
            && name.as_bytes()[3].is_ascii_digit())
}

pub fn new_id(prefix: &str) -> Result<String, String> {
    let mut bytes = [0u8; 16];
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status < 0 {
        return Err("Cannot allocate a session identifier".into());
    }
    use std::fmt::Write;
    let mut result = format!("{prefix}_");
    for byte in bytes {
        write!(result, "{byte:02x}").expect("writing to String");
    }
    Ok(result)
}

// FNV-1a 128 with length-prefixed components keeps identifiers stable across
// Rust versions. Media integrity is checked separately with SHA-256.
pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut hash = 0x6c62272e07bb014262b821756295c58du128;
    for part in std::iter::once(&prefix).chain(parts) {
        for b in (part.len() as u64)
            .to_le_bytes()
            .into_iter()
            .chain(part.bytes())
        {
            hash ^= u128::from(b);
            hash = hash.wrapping_mul(0x0000000001000000000000000000013b);
        }
    }
    format!("{prefix}_{hash:032x}")
}
