//! Portable settings for live and guided batch transcription.
use crate::live_wire::{QUALIFIED_LIVE_GATE, QUALIFIED_LIVE_SPEAKERS};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Component, Path, PathBuf};

pub const VERSION: u32 = 1;
pub const FILENAME: &str = "transcription.json";
pub const GUI_CONFIG_ENV: &str = "AUDIOCAPTURE_GUI_CONFIG";
pub const GUI_PID_ENV: &str = "AUDIOCAPTURE_GUI_PID";
pub const MAX_ENDPOINT_UNITS: usize = 2048;
pub const MAX_FIELD_UNITS: usize = 256;
pub const MAX_PATH_UNITS: usize = 32_767;
const MAX_CONFIG_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionConfig {
    pub version: u32,
    pub endpoint: String,
    pub model: String,
    #[serde(default)]
    pub api_key_env: String,
    #[serde(default)]
    pub transcript_output_folder: PathBuf,
    #[serde(default)]
    pub request_segment_timestamps: bool,
    #[serde(default)]
    pub speech_gating: bool,
    #[serde(default)]
    pub speaker_labels: bool,
    #[serde(default)]
    pub speaker_adapter_config: PathBuf,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            version: VERSION,
            endpoint: String::new(),
            model: String::new(),
            api_key_env: String::new(),
            transcript_output_folder: PathBuf::new(),
            request_segment_timestamps: false,
            speech_gating: false,
            speaker_labels: false,
            speaker_adapter_config: PathBuf::new(),
        }
    }
}

impl TranscriptionConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        let path = absolute_config_path(path)?;
        let file = File::open(&path)
            .map_err(|e| format!("Cannot open transcription config {}: {e}", path.display()))?;
        Self::read(&path, file)
    }

    pub fn load_optional_next_to(exe: &Path) -> Result<Option<Self>, String> {
        let mut path = absolute_config_path(exe)?;
        path.set_file_name(FILENAME);
        match File::open(&path) {
            Ok(file) => Self::read(&path, file).map(Some),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!(
                "Cannot open transcription config {}: {e}",
                path.display()
            )),
        }
    }

    fn read(path: &Path, file: File) -> Result<Self, String> {
        let mut bytes = Vec::new();
        file.take((MAX_CONFIG_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Cannot read transcription config {}: {e}", path.display()))?;
        Self::decode(&bytes, config_dir(path)?)
    }

    /// Compact snapshot of the recorder's current fields for a guided worker.
    pub fn to_gui_snapshot(&self) -> Result<String, String> {
        self.validate()?;
        let json = serde_json::to_string(self)
            .map_err(|_| "Cannot serialize transcription config".to_owned())?;
        check_size(json.len())?;
        Ok(json)
    }

    pub fn from_gui_snapshot(snapshot: &str) -> Result<Self, String> {
        let dir = std::env::current_dir().map_err(|e| e.to_string())?;
        Self::decode(snapshot.as_bytes(), &dir)
    }

    fn decode(bytes: &[u8], dir: &Path) -> Result<Self, String> {
        check_size(bytes.len())?;
        let json = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
        let mut config: Self = serde_json::from_slice(json).map_err(|e| {
            // A parser error can contain user-supplied values or field names.
            format!(
                "Invalid transcription config at line {}, column {}. Check the fields and types against the example.",
                e.line(), e.column()
            )
        })?;
        config.resolve_paths(dir)?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let path = absolute_config_path(path)?;
        let dir = config_dir(&path)?;
        let mut config = self.clone();
        config.resolve_paths(dir)?;
        for value in [
            &mut config.transcript_output_folder,
            &mut config.speaker_adapter_config,
        ] {
            if let Ok(relative) = value.strip_prefix(dir) {
                *value = if relative.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    relative.to_path_buf()
                };
            }
        }
        let mut bytes = serde_json::to_vec_pretty(&config)
            .map_err(|_| "Cannot serialize transcription config".to_owned())?;
        bytes.push(b'\n');
        check_size(bytes.len())?;
        crate::prefs::write_atomic(&path, &bytes)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != VERSION {
            return Err(format!("Use transcription config version {VERSION}."));
        }
        validate_connection(&self.endpoint, &self.model, &self.api_key_env)?;
        if self.speech_gating && !QUALIFIED_LIVE_GATE {
            return Err("Speech gating is evaluation only. Set speech_gating to false.".into());
        }
        if self.speaker_labels && !QUALIFIED_LIVE_SPEAKERS {
            return Err(
                "Live speaker labels are evaluation only. Set speaker_labels to false.".into(),
            );
        }
        if self.speaker_labels && self.speaker_adapter_config.as_os_str().is_empty() {
            return Err("Set speaker_adapter_config to use live speaker labels.".into());
        }
        validate_path(&self.transcript_output_folder, "Transcript output folder")?;
        validate_path(&self.speaker_adapter_config, "Speaker adapter config")
    }

    fn resolve_paths(&mut self, dir: &Path) -> Result<(), String> {
        self.validate()?;
        self.transcript_output_folder = resolve_path(dir, &self.transcript_output_folder)?;
        self.speaker_adapter_config = resolve_path(dir, &self.speaker_adapter_config)?;
        validate_path(&self.transcript_output_folder, "Transcript output folder")?;
        validate_path(&self.speaker_adapter_config, "Speaker adapter config")
    }
}

pub fn validate_connection(endpoint: &str, model: &str, key_env: &str) -> Result<(), String> {
    if model.trim().is_empty()
        || model.encode_utf16().count() > MAX_FIELD_UNITS
        || model.chars().any(char::is_control)
    {
        return Err("Enter a recognition model of up to 256 characters.".into());
    }
    // The worker's live argument reader also has a UTF-8 byte limit.
    if endpoint.len() > 4096 || endpoint.encode_utf16().count() > MAX_ENDPOINT_UNITS {
        return Err("The transcription base URL is too long.".into());
    }
    crate::transcription_endpoint::endpoint(endpoint, false)?;
    if !key_env.is_empty()
        && (key_env.len() > MAX_FIELD_UNITS
            || key_env.as_bytes()[0].is_ascii_digit()
            || !key_env
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_'))
    {
        return Err(
            "Enter only the name of the environment variable that contains the API key.".into(),
        );
    }
    if is_gui_environment_variable(key_env.as_ref()) {
        return Err("Choose an API key variable name that the recorder does not reserve.".into());
    }
    Ok(())
}

pub fn is_gui_environment_variable(name: &std::ffi::OsStr) -> bool {
    name.eq_ignore_ascii_case(GUI_CONFIG_ENV) || name.eq_ignore_ascii_case(GUI_PID_ENV)
}

fn check_size(size: usize) -> Result<(), String> {
    if size > MAX_CONFIG_BYTES {
        return Err("Transcription config exceeds its 256 KiB size limit.".into());
    }
    Ok(())
}

fn absolute_config_path(path: &Path) -> Result<PathBuf, String> {
    if path.file_name().is_none() {
        return Err("Choose a transcription config file.".into());
    }
    std::path::absolute(path).map_err(|e| format!("Cannot resolve transcription config: {e}"))
}

fn config_dir(path: &Path) -> Result<&Path, String> {
    path.parent()
        .ok_or_else(|| "Transcription config needs a parent folder.".into())
}

fn resolve_path(dir: &Path, value: &Path) -> Result<PathBuf, String> {
    if value.as_os_str().is_empty() || value.is_absolute() {
        return Ok(value.to_path_buf());
    }
    std::path::absolute(dir.join(value)).map_err(|e| format!("Cannot resolve config path: {e}"))
}

fn validate_path(path: &Path, label: &str) -> Result<(), String> {
    let value = path
        .to_str()
        .ok_or_else(|| format!("{label} is not valid Unicode."))?;
    if value.encode_utf16().count() > MAX_PATH_UNITS || value.chars().any(char::is_control) {
        return Err(format!(
            "{label} contains invalid characters or is too long."
        ));
    }
    if !path.is_absolute()
        && matches!(
            path.components().next(),
            Some(Component::Prefix(_) | Component::RootDir)
        )
    {
        return Err(format!("Enter a full or relative path for {label}."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "ac-transcription-config-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn example() -> TranscriptionConfig {
        TranscriptionConfig {
            endpoint: "http://127.0.0.1:9000/v1".into(),
            model: "local-asr".into(),
            ..Default::default()
        }
    }

    #[test]
    fn relative_paths_stay_portable_across_save_and_reload() {
        let dir = Temp::new();
        let file = dir.0.join(FILENAME);
        let config = TranscriptionConfig {
            api_key_env: "EXAMPLE_UNSET_TRANSCRIPTION_KEY".into(),
            transcript_output_folder: "transcripts".into(),
            request_segment_timestamps: true,
            speaker_adapter_config: "models/adapter.json".into(),
            ..example()
        };
        config.save(&file).unwrap();
        let loaded = TranscriptionConfig::load(&file).unwrap();
        assert_eq!(loaded.transcript_output_folder, dir.0.join("transcripts"));
        assert_eq!(
            loaded.speaker_adapter_config,
            dir.0.join("models/adapter.json")
        );
        assert!(loaded.request_segment_timestamps);
        assert_eq!(loaded.api_key_env, config.api_key_env);
        loaded.save(&file).unwrap();
        let written: TranscriptionConfig =
            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(
            written.transcript_output_folder,
            config.transcript_output_folder
        );
        assert_eq!(TranscriptionConfig::load(&file).unwrap(), loaded);

        let elsewhere = dir.0.join("other").join(FILENAME);
        loaded.save(&elsewhere).unwrap();
        assert_eq!(TranscriptionConfig::load(&elsewhere).unwrap(), loaded);
    }

    #[test]
    fn optional_sibling_accepts_bom_and_rejects_unsupported_configs() {
        let dir = Temp::new();
        let exe = dir.0.join("audio-capture.exe");
        let file = dir.0.join(FILENAME);
        assert!(
            TranscriptionConfig::load_optional_next_to(&exe)
                .unwrap()
                .is_none()
        );
        let mut bytes = vec![0xef, 0xbb, 0xbf];
        bytes
            .extend_from_slice(br#"{"version":1,"endpoint":"http://[::1]:80/v1","model":"local"}"#);
        fs::write(&file, bytes).unwrap();
        let config = TranscriptionConfig::load_optional_next_to(&exe)
            .unwrap()
            .unwrap();
        assert!(config.transcript_output_folder.as_os_str().is_empty());
        assert!(!config.speaker_labels);
        assert!(!config.speech_gating);

        for invalid in [
            br#"{"version":2,"endpoint":"https://example.com/v1","model":"test"}"#.as_slice(),
            br#"{"endpoint":"https://example.com/v1","model":"test"}"#,
            br#"{"version":1,"endpoint":"https://example.com/v1","model":"test","api_key":"literal-secret"}"#,
            br#"{"version":1,"endpoint":"https://example.com/v1","model":"test","version":1}"#,
        ] {
            fs::write(&file, invalid).unwrap();
            let error = TranscriptionConfig::load_optional_next_to(&exe).unwrap_err();
            assert!(!error.contains("literal-secret"));
        }
    }

    #[test]
    fn invalid_config_does_not_replace_a_valid_file() {
        let dir = Temp::new();
        let file = dir.0.join(FILENAME);
        let mut config = example();
        config.save(&file).unwrap();
        let valid = fs::read(&file).unwrap();
        config.api_key_env = "sk-literal-secret".into();
        assert!(!config.save(&file).unwrap_err().contains("literal-secret"));
        assert_eq!(fs::read(&file).unwrap(), valid);
        fs::write(&file, vec![b' '; MAX_CONFIG_BYTES + 1]).unwrap();
        assert!(
            TranscriptionConfig::load(&file)
                .unwrap_err()
                .contains("size limit")
        );
    }

    #[test]
    fn validation_matches_gui_limits_and_worker_endpoint_rules() {
        let mut config = example();
        for endpoint in [
            "http://example.com/v1",
            "https://user:literal-secret@example.com/v1",
            "https://example.com/v1?token=literal-secret",
            "https://example.com:0/v1",
            "https://example.com/../v1",
            "https:///v1",
        ] {
            config.endpoint = endpoint.into();
            assert!(!config.validate().unwrap_err().contains("literal-secret"));
        }
        config.endpoint = "http://localhost:9080/proxy/v1".into();
        config.validate().unwrap();
        config.model = "😀".repeat(MAX_FIELD_UNITS / 2 + 1);
        assert!(config.validate().is_err());
        config.model = "local-asr".into();
        config.transcript_output_folder = "dir\nname".into();
        assert!(config.validate().is_err());

        let config = TranscriptionConfig {
            speech_gating: true,
            ..example()
        };
        assert_eq!(config.validate().is_ok(), QUALIFIED_LIVE_GATE);
        let mut config = TranscriptionConfig {
            speaker_labels: true,
            ..example()
        };
        assert!(config.validate().is_err());
        config.speaker_adapter_config = "model.json".into();
        assert_eq!(config.validate().is_ok(), QUALIFIED_LIVE_SPEAKERS);
    }
}
