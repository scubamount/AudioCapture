use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const VERSION: u32 = 1;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub format: String,
    pub wav_rate: u32,
    pub channels: u16,
    pub mp3_kbps: u32,
    pub opus_kbps: u32,
    pub flac_level: u8,
    pub folder: PathBuf,
    pub separate: bool,
    pub mixed: bool,
    pub skip_silence: bool,
    pub active_only: bool,
    pub process_names: Vec<String>,
    pub system: bool,
    pub mic: Option<String>,
    pub monitor: Option<String>,
    pub monitor_only: bool,
    pub app_gain: f32,
    pub mic_gain: f32,
}

impl Default for Settings {
    fn default() -> Self {
        let folder = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Documents")
            .join("AudioCaptures");
        Self {
            format: "wav".into(),
            wav_rate: 0,
            channels: 2,
            mp3_kbps: 192,
            opus_kbps: 128,
            flac_level: 5,
            folder,
            separate: true,
            mixed: false,
            skip_silence: false,
            active_only: true,
            process_names: Vec::new(),
            system: false,
            mic: None,
            monitor: None,
            monitor_only: false,
            app_gain: 1.0,
            mic_gain: 1.0,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    settings: Settings,
}

fn root() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|p| p.join("AudioCaptureRust"))
        .ok_or_else(|| "LOCALAPPDATA is unavailable".into())
}

fn legacy_root() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|p| p.join("AudioCapture"))
        .ok_or_else(|| "LOCALAPPDATA is unavailable".into())
}

fn validate(s: &Settings) -> Result<(), String> {
    if !["wav", "mp3", "opus", "flac"].contains(&s.format.as_str())
        || ![0, 16_000, 44_100, 48_000].contains(&s.wav_rate)
        || ![1, 2].contains(&s.channels)
        || ![128, 192, 256, 320].contains(&s.mp3_kbps)
        || ![24, 32, 48, 64, 96, 128, 192, 256].contains(&s.opus_kbps)
        || s.flac_level > 8
        || !s.app_gain.is_finite()
        || !(0.0..=1.0).contains(&s.app_gain)
        || !s.mic_gain.is_finite()
        || !(0.0..=1.0).contains(&s.mic_gain)
        || (!s.separate && !s.mixed && !s.monitor_only)
    {
        return Err("Invalid recorder settings".into());
    }
    Ok(())
}

fn read_current(path: &Path) -> Result<Settings, String> {
    let bytes = fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let stored: Stored = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Invalid settings in {}: {e}", path.display()))?;
    if stored.version != VERSION {
        return Err(format!(
            "Unsupported settings version {} in {}",
            stored.version,
            path.display()
        ));
    }
    validate(&stored.settings)?;
    Ok(stored.settings)
}

fn write_current(path: &Path, settings: &Settings) -> Result<(), String> {
    validate(settings)?;
    let dir = path.parent().ok_or("Settings destination has no parent")?;
    fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    let data = serde_json::to_vec_pretty(&Stored {
        version: VERSION,
        settings: settings.clone(),
    })
    .map_err(|e| e.to_string())?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!(".settings-{}-{sequence}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| format!("Cannot create temporary settings file: {e}"))?;
    let result = (|| {
        file.write_all(&data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&temp);
        return Err(format!("Cannot save {}: {e}", path.display()));
    }
    Ok(())
}

fn optional_file(path: &Path) -> Result<bool, String> {
    match fs::metadata(path) {
        Ok(meta) => Ok(meta.is_file()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("Cannot inspect {}: {e}", path.display())),
    }
}

fn legacy(path: &Path) -> Result<Settings, String> {
    let bytes = fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let v: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Invalid legacy settings in {}: {e}", path.display()))?;
    if !v.is_object() {
        return Err(format!("Invalid legacy settings in {}", path.display()));
    }
    let mut s = Settings::default();
    if let Some(p) = v["outputPath"].as_str() {
        s.folder = PathBuf::from(p);
    }
    if let Some(i) = v["format"].as_u64() {
        if let Some(f) = ["wav", "mp3", "opus", "flac"].get(i as usize) {
            s.format = (*f).into();
        }
    }
    if let Some(i) = v["mp3Bitrate"].as_u64() {
        if let Some(n) = [128, 192, 256, 320].get(i as usize) {
            s.mp3_kbps = *n;
        }
    }
    if let Some(i) = v["opusBitrate"].as_u64() {
        if let Some(n) = [64, 96, 128, 192, 256].get(i as usize) {
            s.opus_kbps = *n;
        }
    }
    if let Some(n) = v["flacCompression"].as_u64().filter(|n| *n <= 8) {
        s.flac_level = n as u8;
    }
    if let Some(value) = v["skipSilence"].as_bool() {
        s.skip_silence = value;
    }
    if let Some(value) = v["monitorOnly"].as_bool() {
        s.monitor_only = value;
    }
    if let Some(mode) = v["recordingMode"].as_u64() {
        match mode {
            0 => {
                s.separate = true;
                s.mixed = false;
            }
            1 => {
                s.separate = false;
                s.mixed = true;
            }
            2 => {
                s.separate = true;
                s.mixed = true;
            }
            _ => {}
        }
    }
    if let Some(n) = v["processVolume"]
        .as_f64()
        .filter(|n| (0.0..=100.0).contains(n))
    {
        s.app_gain = n as f32 / 100.0;
    }
    if let Some(n) = v["microphoneVolume"]
        .as_f64()
        .filter(|n| (0.0..=100.0).contains(n))
    {
        s.mic_gain = n as f32 / 100.0;
    }
    if let Some(names) = v["processNames"].as_array() {
        for name in names.iter().filter_map(Value::as_str) {
            if name == "[System Audio - All Processes]" {
                s.system = true;
            } else {
                s.process_names.push(name.to_owned());
            }
        }
    }
    if v["captureMicrophone"].as_bool() == Some(true) {
        if v["microphoneDeviceIds"]
            .as_array()
            .is_some_and(|devices| devices.len() > 1)
        {
            return Err("Legacy preset contains multiple microphone devices. Select one microphone before recording.".into());
        }
        // Empty ID requires a deliberate choice when the legacy file has no stable ID.
        s.mic = Some(
            v["microphoneDeviceIds"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        );
    }
    if v["passthrough"].as_bool() == Some(true) {
        // A legacy index is not stable across device enumeration.
        s.monitor = Some(String::new());
    }
    validate(&s)?;
    Ok(s)
}

fn valid_name(name: &str) -> Result<(), String> {
    let upper = name.to_uppercase();
    let stem = upper.split('.').next().unwrap_or("");
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if name.is_empty()
        || name.len() > 180
        || name.ends_with([' ', '.'])
        || name
            .chars()
            .any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c))
        || reserved.contains(&stem)
    {
        return Err("Invalid preset name".into());
    }
    Ok(())
}

fn preset_path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    valid_name(name)?;
    Ok(dir.join("presets").join(format!("{name}.json")))
}

pub fn load() -> Result<Settings, String> {
    let path = root()?.join("settings.json");
    if optional_file(&path)? {
        return read_current(&path);
    }
    let old = legacy_root()?.join("settings.json");
    if optional_file(&old)? {
        legacy(&old)
    } else {
        Ok(Settings::default())
    }
}

pub fn save(settings: &Settings) -> Result<(), String> {
    write_current(&root()?.join("settings.json"), settings)
}

fn tombstones_path(base: &Path) -> PathBuf {
    base.join("hidden-legacy-presets.json")
}

fn tombstones(base: &Path) -> Result<Vec<String>, String> {
    let path = tombstones_path(base);
    match fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| format!("Invalid {}: {e}", path.display()))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
    }
}

fn enumerate(dir: &Path, names: &mut BTreeMap<String, String>) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("Cannot read {}: {e}", dir.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        let path = entry.path();
        if path
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| x.eq_ignore_ascii_case("json"))
            != Some(true)
        {
            continue;
        }
        if let Some(name) = path.file_stem().and_then(|x| x.to_str()) {
            if valid_name(name).is_ok() {
                names.insert(name.to_lowercase(), name.to_owned());
            }
        }
    }
    Ok(())
}

pub fn presets() -> Result<Vec<String>, String> {
    let base = root()?;
    let mut names = BTreeMap::new();
    enumerate(&legacy_root()?.join("presets"), &mut names)?;
    for hidden in tombstones(&base)? {
        names.remove(&hidden.to_lowercase());
    }
    enumerate(&base.join("presets"), &mut names)?;
    Ok(names.into_values().collect())
}

pub fn load_preset(name: &str) -> Result<Settings, String> {
    let base = root()?;
    let path = preset_path(&base, name)?;
    if optional_file(&path)? {
        return read_current(&path);
    }
    if tombstones(&base)?
        .iter()
        .any(|n| n.eq_ignore_ascii_case(name))
    {
        return Err("Preset not found".into());
    }
    legacy(&preset_path(&legacy_root()?, name)?)
}

pub fn save_preset(name: &str, settings: &Settings) -> Result<(), String> {
    write_current(&preset_path(&root()?, name)?, settings)
}

pub fn delete_preset(name: &str) -> Result<(), String> {
    let base = root()?;
    let path = preset_path(&base, name)?;
    let legacy_path = preset_path(&legacy_root()?, name)?;
    let new_exists = optional_file(&path)?;
    let old_exists = optional_file(&legacy_path)?;
    if !new_exists && !old_exists {
        return Err("Preset not found".into());
    }
    if old_exists {
        let mut hidden = tombstones(&base)?;
        if !hidden.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            hidden.push(name.to_owned());
        }
        // The separate tombstone file uses the same atomic replacement strategy.
        fs::create_dir_all(&base).map_err(|e| e.to_string())?;
        let data = serde_json::to_vec_pretty(&hidden).map_err(|e| e.to_string())?;
        let tmp = base.join(format!(
            ".hidden-{}-{}.tmp",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            file.write_all(&data)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp, tombstones_path(&base))
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(&tmp);
            return Err(format!("Cannot hide legacy preset: {e}"));
        }
    }
    if new_exists {
        fs::remove_file(&path).map_err(|e| format!("Cannot delete {}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_values_and_source_identity_survive_import() {
        let dir = std::env::temp_dir().join(format!("audiocapture-legacy-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("legacy.json");
        fs::write(&path, r#"{"format":2,"opusBitrate":0,"recordingMode":0,"outputPath":"G:\\audio","processNames":["discord.exe"],"captureMicrophone":true}"#).unwrap();
        let s = legacy(&path).unwrap();
        assert_eq!(s.format, "opus");
        assert_eq!(s.opus_kbps, 64);
        assert!(s.separate && !s.mixed);
        assert_eq!(s.process_names, ["discord.exe"]);
        assert_eq!(s.mic.as_deref(), Some(""));
        assert!(fs::read_to_string(&path).unwrap().contains("discord.exe"));
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn names_and_ranges_are_checked() {
        for name in ["", "..", "CON", "x/y", "trailing.", "bad*"] {
            assert!(valid_name(name).is_err());
        }
        assert!(valid_name("Réunion 音声").is_ok());
        let mut s = Settings::default();
        s.wav_rate = 12_345;
        assert!(validate(&s).is_err());
        s.wav_rate = 16_000;
        assert!(validate(&s).is_ok());
    }

    #[test]
    fn malformed_legacy_data_is_reported() {
        let path = std::env::temp_dir().join(format!(
            "audiocapture-malformed-{}.json",
            std::process::id()
        ));
        fs::write(&path, b"{").unwrap();
        assert!(legacy(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn settings_replace_atomically_and_preserve_unicode() {
        let dir =
            std::env::temp_dir().join(format!("audiocapture-settings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let mut s = Settings::default();
        s.process_names = vec!["音声.exe".into()];
        write_current(&path, &s).unwrap();
        s.opus_kbps = 48;
        write_current(&path, &s).unwrap();
        assert_eq!(read_current(&path).unwrap(), s);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn compact_speech_settings_round_trip() {
        let dir = std::env::temp_dir().join(format!("audiocapture-compact-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let mut settings = Settings::default();
        settings.format = "opus".to_owned();
        settings.opus_kbps = 24;
        settings.channels = 1;
        write_current(&path, &settings).unwrap();
        assert_eq!(read_current(&path).unwrap(), settings);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn multiple_legacy_microphones_require_resolution() {
        let path = std::env::temp_dir().join(format!(
            "audiocapture-multi-mic-{}.json",
            std::process::id()
        ));
        let original = r#"{"captureMicrophone":true,"microphoneDeviceIds":["id-one","id-two"]}"#;
        fs::write(&path, original).unwrap();
        let error = legacy(&path).unwrap_err();
        assert!(error.contains("multiple microphone"));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        fs::remove_file(path).unwrap();
    }
}
