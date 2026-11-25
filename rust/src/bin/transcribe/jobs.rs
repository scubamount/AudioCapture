use crate::{chunks, review, transport};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use windows_sys::Win32::Security::Cryptography::{
    CALG_SHA_256, CRYPT_VERIFYCONTEXT, CryptAcquireContextW, CryptCreateHash, CryptDestroyHash,
    CryptGetHashParam, CryptHashData, CryptReleaseContext, HP_HASHVAL, PROV_RSA_AES,
};
use windows_sys::Win32::Storage::FileSystem::MoveFileW;

pub struct Options {
    pub base_url: String,
    pub model: String,
    pub output_dir: PathBuf,
    pub key_env: Option<String>,
    pub language: Option<String>,
    pub max_file_mib: u64,
    pub allow_http: bool,
    pub timestamps: bool,
    pub provider_profile: Option<transport::ProviderProfile>,
    pub retry_unknown: bool,
    pub same_session: bool,
    pub allow_duplicate_sources: bool,
    pub chunk_long: bool,
    pub review: Option<PathBuf>,
    pub files: Vec<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            model: String::new(),
            output_dir: PathBuf::new(),
            key_env: None,
            language: None,
            max_file_mib: 25,
            allow_http: false,
            timestamps: false,
            provider_profile: None,
            retry_unknown: false,
            same_session: false,
            allow_duplicate_sources: false,
            chunk_long: false,
            review: None,
            files: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Job {
    version: u8,
    #[serde(default)]
    identity: String,
    status: String,
    input: PathBuf,
    source_sha256: String,
    endpoint: String,
    model: String,
    language: Option<String>,
    timestamps: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_profile: Option<transport::ProviderProfile>,
    text: PathBuf,
    json: Option<PathBuf>,
    #[serde(default)]
    chunk_offset_frames: Option<u64>,
    #[serde(default)]
    chunk_frames: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct CachedResponse {
    content_type: String,
    body: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct Aggregate {
    identity: String,
    text: PathBuf,
    json: PathBuf,
}

struct PartOutput {
    track: review::ReviewTrack,
    speaker_segments: Vec<SpeakerSegment>,
}

#[derive(Serialize)]
struct SpeakerSegment {
    start: Option<f64>,
    end: Option<f64>,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    speaker: Option<String>,
}

// Windows' built-in cryptographic provider avoids buffering entire recordings
// and identifies jobs independently of input names and modification times.
pub(super) fn digest(mut input: impl Read) -> Result<String, String> {
    let mut provider = 0usize;
    let mut hash = 0usize;
    unsafe {
        if CryptAcquireContextW(
            &mut provider,
            std::ptr::null(),
            std::ptr::null(),
            PROV_RSA_AES,
            CRYPT_VERIFYCONTEXT,
        ) == 0
        {
            return Err("Cannot initialize the Windows SHA-256 provider".into());
        }
        if CryptCreateHash(provider, CALG_SHA_256, 0, 0, &mut hash) == 0 {
            CryptReleaseContext(provider, 0);
            return Err("Cannot create SHA-256 hash".into());
        }
    }
    struct Guard(usize, usize);
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                CryptDestroyHash(self.1);
                CryptReleaseContext(self.0, 0);
            }
        }
    }
    let _guard = Guard(provider, hash);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = input
            .read(&mut buffer)
            .map_err(|e| format!("Cannot read input for hashing: {e}"))?;
        if n == 0 {
            break;
        }
        if unsafe { CryptHashData(hash, buffer.as_ptr(), n as u32, 0) } == 0 {
            return Err("Cannot hash input".into());
        }
    }
    let mut bytes = [0u8; 32];
    let mut length = bytes.len() as u32;
    if unsafe { CryptGetHashParam(hash, HP_HASHVAL, bytes.as_mut_ptr(), &mut length, 0) } == 0
        || length != 32
    {
        return Err("Cannot retrieve SHA-256 hash".into());
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn read_job_json<T: serde::de::DeserializeOwned>(input: impl Read) -> serde_json::Result<T> {
    serde_json::from_reader(BufReader::new(input))
}

fn save_state(path: &Path, job: &Job) -> Result<(), String> {
    let temp = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp)
        .map_err(|e| format!("Cannot write job state: {e}"))?;
    serde_json::to_writer(&mut file, job).map_err(|e| e.to_string())?;
    file.flush()
        .map_err(|e| format!("Cannot flush job state: {e}"))?;
    file.sync_all()
        .map_err(|e| format!("Cannot sync job state: {e}"))?;
    fs::rename(temp, path).map_err(|e| format!("Cannot save job state: {e}"))
}

pub(super) fn lock(path: &Path) -> Result<File, String> {
    // Exclusive Windows sharing releases the lock on process exit, even after a crash.
    OpenOptions::new()
        .write(true)
        .create(true)
        .share_mode(0)
        .open(path)
        .map_err(|e| format!("Cannot lock job {}: {e}", path.display()))
}

fn parse_response(
    response: &transport::Response,
) -> Result<(String, Option<serde_json::Value>), String> {
    let content_type = response
        .content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if content_type == "text/plain" {
        return String::from_utf8(response.body.clone())
            .map(|s| (s, None))
            .map_err(|_| "Response text is not UTF-8".into());
    }
    if content_type != "application/json" && !content_type.ends_with("+json") {
        return Err("Unsupported response content type".into());
    }
    let value: serde_json::Value = serde_json::from_slice(&response.body)
        .map_err(|_| "Invalid JSON transcription response")?;
    let text = value
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or("The JSON transcription response has no text field")?
        .to_owned();
    Ok((text, Some(value)))
}

pub(super) fn output_duration(path: &Path) -> Option<f64> {
    // Duration is optional. Never assign timing based on an unavailable probe.
    let result = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-format_whitelist",
            chunks::MEDIA_DEMUXERS,
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            "-i",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !result.status.success() {
        return None;
    }
    let duration = String::from_utf8(result.stdout)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()?;
    (duration.is_finite() && duration > 0.0).then_some(duration)
}

fn segments(raw: Option<&serde_json::Value>, duration: Option<f64>) -> Vec<review::ReviewSegment> {
    let Some(duration) = duration else {
        return Vec::new();
    };
    let Some(items) = raw
        .and_then(|v| v.get("segments"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for item in items {
        let (Some(start), Some(end), Some(text)) = (
            item.get("start").and_then(|v| v.as_f64()),
            item.get("end").and_then(|v| v.as_f64()),
            item.get("text").and_then(|v| v.as_str()),
        ) else {
            return Vec::new();
        };
        if !start.is_finite()
            || !end.is_finite()
            || start < 0.0
            || end < start
            || end > duration + 0.1
        {
            return Vec::new();
        }
        result.push(review::ReviewSegment {
            start,
            end,
            text: text.to_owned(),
        });
    }
    // Segment-only views must not hide unsegmented words returned in text.
    let source = raw
        .and_then(|v| v.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let joined = result
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    if joined.split_whitespace().collect::<Vec<_>>().join(" ")
        != source.split_whitespace().collect::<Vec<_>>().join(" ")
    {
        return Vec::new();
    }
    result
}

fn review_segments(
    options: &Options,
    raw: Option<&serde_json::Value>,
    upload: &Path,
    part: Option<(u64, u64)>,
) -> Vec<review::ReviewSegment> {
    if (options.review.is_none() && part.is_none()) || raw.and_then(|v| v.get("segments")).is_none()
    {
        return Vec::new();
    }
    let duration = part
        .map(|(_, frames)| frames as f64 / 16_000.0)
        .or_else(|| output_duration(upload));
    segments(raw, duration)
}

fn speaker_segments(raw: Option<&serde_json::Value>, duration: f64) -> Vec<SpeakerSegment> {
    let Some(items) = raw
        .and_then(|value| value.get("segments"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let text = item.get("text")?.as_str()?;
            let times = item
                .get("start")
                .and_then(serde_json::Value::as_f64)
                .zip(item.get("end").and_then(serde_json::Value::as_f64))
                .filter(|&(start, end)| {
                    start.is_finite()
                        && end.is_finite()
                        && start >= 0.0
                        && end >= start
                        && end <= duration
                });
            let speaker = times
                .filter(|&(start, end)| start < end)
                .and_then(|_| item.get("speaker")?.as_str())
                .filter(|speaker| {
                    !speaker.trim().is_empty()
                        && speaker.len() <= 256
                        && !speaker.chars().any(char::is_control)
                })
                .map(str::to_owned);
            let (start, end) = times.unzip();
            Some(SpeakerSegment {
                start,
                end,
                text: text.to_owned(),
                speaker,
            })
        })
        .collect()
}

fn request_key<T: Serialize>(
    legacy: &T,
    profile: Option<transport::ProviderProfile>,
) -> Result<String, String> {
    let identity = match profile {
        Some(profile) => serde_json::to_vec(&("provider-profile", profile, legacy)),
        None => serde_json::to_vec(legacy),
    }
    .map_err(|error| error.to_string())?;
    digest(identity.as_slice())
}

pub(super) fn is_media(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            audio_capture::TRANSCRIPTION_EXTENSIONS
                .iter()
                .any(|allowed| extension.eq_ignore_ascii_case(allowed))
        })
}

fn safe_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    let safe: String = stem
        .chars()
        .take(80)
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let safe = safe.trim_matches([' ', '.']);
    if safe.is_empty() {
        "recording".into()
    } else {
        safe.into()
    }
}

fn write_new(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
    out.write_all(contents)
        .and_then(|()| out.sync_all())
        .map_err(|e| format!("Cannot save {}: {e}", path.display()))
}

fn publish_aggregate(
    directory: &Path,
    base: &str,
    text: &[u8],
    manifest: &[u8],
) -> Result<(PathBuf, PathBuf), String> {
    let mut attempt = 1u32;
    loop {
        let prefix = if attempt == 1 {
            format!("{base}-combined")
        } else {
            format!("{base}-{attempt}-combined")
        };
        let text_path = directory.join(format!("{prefix}.txt"));
        let json_path = directory.join(format!("{prefix}.json"));
        let mut collision = false;
        for (path, data) in [(&text_path, text), (&json_path, manifest)] {
            if path.exists() {
                if fs::read(path).map_err(|e| e.to_string())? != data {
                    collision = true;
                    break;
                }
            } else if let Err(error) = stage_and_publish_new(path, data) {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    collision = true;
                    break;
                }
                return Err(format!("Cannot publish {}: {error}", path.display()));
            }
        }
        if !collision {
            return Ok((text_path, json_path));
        }
        attempt = attempt
            .checked_add(1)
            .ok_or("Too many existing aggregate files")?;
    }
}

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn stage_and_publish_new(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    stage_and_publish_with_sequence(path, contents, || {
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    })
}

fn stage_and_publish_with_sequence(
    path: &Path,
    contents: &[u8],
    mut next_sequence: impl FnMut() -> u64,
) -> std::io::Result<()> {
    let mut reserved = None;
    for _ in 0..100 {
        let temp = path.with_extension(format!(
            "pending-{}-{}",
            std::process::id(),
            next_sequence()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => {
                reserved = Some((temp, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let (temp, mut file) = reserved.ok_or_else(|| {
        std::io::Error::other("Cannot find an unused temporary file after 100 attempts")
    })?;
    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    drop(file);
    let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // MoveFileW publishes on the same volume and refuses an existing destination.
    if unsafe { MoveFileW(from.as_ptr(), to.as_ptr()) } == 0 {
        let error = std::io::Error::last_os_error();
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

fn publish_new(path: &Path, contents: &[u8]) -> Result<(), String> {
    stage_and_publish_new(path, contents)
        .map_err(|error| format!("Cannot publish saved response {}: {error}", path.display()))
}

fn disambiguate_labels(tracks: &mut [review::ReviewTrack]) {
    let labels: Vec<String> = tracks.iter().map(|track| track.label.clone()).collect();
    for track in tracks {
        if labels.iter().filter(|label| *label == &track.label).count() > 1 {
            track.label = format!("{} ({})", track.label, track.audio.display());
        }
    }
}

fn valid_output_path(path: &Path, directory: &Path, prefix: &str, extension: &str) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if path.parent() != Some(directory) || !name.starts_with(prefix) || !name.ends_with(extension) {
        return false;
    }
    if path.exists() {
        return fs::canonicalize(path)
            .ok()
            .is_some_and(|canonical| canonical.parent() == Some(directory));
    }
    true
}

fn recorder_session(path: &Path) -> Option<(&str, bool)> {
    let stem = path.file_stem()?.to_str()?;
    let (name, id) = stem.rsplit_once('-')?;
    if id.len() < 15 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((id, name == "Combined"))
}

fn validate_batch(options: &Options) -> Result<(), String> {
    if options.timestamps && options.provider_profile.is_some() {
        return Err("Choose either --timestamps or --provider-profile".into());
    }
    if options.review.as_ref().is_some_and(|path| path.exists()) {
        return Err("The requested review file already exists. Choose a new path.".into());
    }
    if !options.allow_duplicate_sources {
        for left in &options.files {
            if let Some((id, true)) = recorder_session(left) {
                if options.files.iter().any(
                    |right| matches!(recorder_session(right), Some((other, false)) if other == id),
                ) {
                    return Err("Combined and separate tracks contain duplicate audio. Select one or use --allow-duplicate-sources".into());
                }
            }
        }
    }
    Ok(())
}

pub fn run(mut options: Options) -> Result<(), String> {
    validate_batch(&options)?;
    let endpoint = transport::endpoint(&options.base_url, options.allow_http)?;
    let key = match &options.key_env {
        Some(name) => Some(
            std::env::var(name).map_err(|_| "The named API key environment variable is missing")?,
        ),
        None => None,
    };
    if key.as_ref().is_some_and(|k| k.is_empty()) {
        return Err("The named API key environment variable is empty".into());
    }
    fs::create_dir_all(&options.output_dir)
        .map_err(|e| format!("Cannot create output directory: {e}"))?;
    options.output_dir = fs::canonicalize(&options.output_dir)
        .map_err(|e| format!("Cannot resolve output directory: {e}"))?;
    let job_dir = options.output_dir.join(".audio-capture-jobs");
    fs::create_dir_all(&job_dir).map_err(|e| format!("Cannot create job directory: {e}"))?;
    let mut failures = 0;
    let mut tracks = Vec::new();
    for input in &options.files {
        match process_file(input, &options, &endpoint, key.as_deref(), &job_dir) {
            Ok(track) => {
                if options.review.is_some() {
                    tracks.push(track);
                }
            }
            Err(error) => {
                eprintln!("{}: {error}", input.display());
                failures += 1;
            }
        }
    }
    if let Some(path) = &options.review {
        disambiguate_labels(&mut tracks);
        if tracks.is_empty() {
            failures += 1;
            eprintln!("No transcripts to review");
        } else if let Err(error) = review::write_review(path, &tracks, options.same_session) {
            eprintln!("Cannot create review: {error}");
            failures += 1;
        }
    }
    if failures > 0 {
        Err(format!(
            "{failures} item(s) failed or need an explicit retry"
        ))
    } else {
        Ok(())
    }
}

fn process_file(
    input: &Path,
    options: &Options,
    endpoint: &transport::Endpoint,
    key: Option<&str>,
    job_dir: &Path,
) -> Result<review::ReviewTrack, String> {
    if !is_media(input) {
        return Err("Input extension is not a recognized audio or media type".into());
    }
    let input = fs::canonicalize(input).map_err(|e| format!("Cannot open input: {e}"))?;
    // Keep the input stable between identification and upload.
    let mut stable = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&input)
        .map_err(|e| format!("Cannot hold input for reading: {e}"))?;
    let metadata = stable.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err("Input must be a nonempty file".into());
    }
    let limit = options.max_file_mib * 1024 * 1024;
    let oversized = metadata.len() > limit;
    if oversized && !options.chunk_long {
        return Err("Input exceeds the configured file limit. Use --chunk-long to convert it into bounded WAV chunks.".into());
    }
    let source_sha256 = digest(&mut stable)?;
    if !oversized {
        return process_part(
            &input,
            &source_sha256,
            &input,
            None,
            options,
            endpoint,
            key,
            job_dir,
        )
        .map(|part| part.track);
    }
    let aggregate_key = request_key(
        &(
            input.to_string_lossy().as_ref(),
            &source_sha256,
            &transport::canonical_key(endpoint),
            &options.model,
            &options.language,
            options.timestamps,
            limit,
        ),
        options.provider_profile,
    )?;
    let aggregate_state = job_dir.join(format!("{aggregate_key}.aggregate.json"));
    let _aggregate_lock = lock(&job_dir.join(format!("{aggregate_key}.aggregate.lock")))?;
    if aggregate_state.exists() {
        let saved = File::open(&aggregate_state)
            .ok()
            .and_then(|file| read_job_json::<Aggregate>(file).ok());
        if let Some(saved) = saved {
            let prefix = format!("{}-", safe_name(&input));
            if saved.identity == aggregate_key
                && valid_output_path(&saved.text, &options.output_dir, &prefix, "-combined.txt")
                && valid_output_path(&saved.json, &options.output_dir, &prefix, "-combined.json")
                && saved.text.is_file()
                && saved.json.is_file()
            {
                let manifest = File::open(&saved.json)
                    .ok()
                    .and_then(|file| read_job_json::<serde_json::Value>(file).ok());
                let reconstructed = manifest
                    .as_ref()
                    .and_then(|v| v.get("parts"))
                    .and_then(|v| v.as_array())
                    .and_then(|parts| {
                        parts
                            .iter()
                            .map(|p| p.get("text")?.as_str().map(str::to_owned))
                            .collect::<Option<Vec<_>>>()
                    })
                    .map(|parts| {
                        parts
                            .into_iter()
                            .filter(|text| !text.is_empty())
                            .collect::<Vec<_>>()
                            .join("\n")
                    });
                let text_matches = reconstructed
                    .as_deref()
                    .zip(fs::read_to_string(&saved.text).ok().as_deref())
                    .is_some_and(|(expected, actual)| expected == actual);
                if manifest
                    .as_ref()
                    .and_then(|v| v.get("source_sha256"))
                    .and_then(|v| v.as_str())
                    == Some(source_sha256.as_str())
                    && manifest
                        .as_ref()
                        .and_then(|value| value.get("provider_profile"))
                        .cloned()
                        .map(serde_json::from_value::<transport::ProviderProfile>)
                        .transpose()
                        .is_ok_and(|profile| profile == options.provider_profile)
                    && text_matches
                {
                    println!(
                        "{} -> {} (already complete)",
                        input.display(),
                        saved.text.display()
                    );
                    let mut timed = Vec::new();
                    let mut untimed = Vec::new();
                    if options.review.is_some() {
                        let mut expected_offset = 0u64;
                        let entries = manifest
                            .as_ref()
                            .and_then(|v| v.get("parts"))
                            .and_then(|v| v.as_array())
                            .ok_or("Aggregate parts are invalid")?;
                        for part in entries {
                            let offset = part
                                .get("offset_frames")
                                .and_then(|v| v.as_u64())
                                .ok_or("Aggregate offset is invalid")?;
                            let frames = part
                                .get("frames")
                                .and_then(|v| v.as_u64())
                                .filter(|n| *n > 0)
                                .ok_or("Aggregate length is invalid")?;
                            if offset != expected_offset {
                                return Err("Aggregate chunk offsets are inconsistent".into());
                            }
                            expected_offset = offset
                                .checked_add(frames)
                                .ok_or("Aggregate offset is too large")?;
                            let text = part
                                .get("text")
                                .and_then(|v| v.as_str())
                                .ok_or("Aggregate text is invalid")?;
                            let part_segments =
                                segments(Some(part), Some(frames as f64 / 16_000.0));
                            if part_segments.is_empty() && !text.trim().is_empty() {
                                untimed.push(text.to_owned());
                            }
                            for segment in part_segments {
                                timed.push(review::ReviewSegment {
                                    start: offset as f64 / 16_000.0 + segment.start,
                                    end: offset as f64 / 16_000.0 + segment.end,
                                    text: segment.text,
                                });
                            }
                        }
                    }
                    return Ok(review::ReviewTrack {
                        label: safe_name(&input),
                        audio: input,
                        text: reconstructed.unwrap_or_default(),
                        segments: timed,
                        untimed,
                    });
                }
            }
        }
        // Missing or damaged aggregate output is rebuilt from per-part jobs.
    }
    let mut parts = Vec::new();
    chunks::for_each_wav(&input, job_dir, limit, |path, offset, frames| {
        let track = process_part(
            &input,
            &source_sha256,
            path,
            Some((offset, frames)),
            options,
            endpoint,
            key,
            job_dir,
        )
        .map_err(|error| {
            format!(
                "Chunk at frame {offset} ({} seconds): {error}",
                offset as f64 / 16_000.0
            )
        })?;
        parts.push((offset, frames, track));
        Ok(())
    })?;
    if parts.is_empty() {
        return Err("The input produced no audio chunks".into());
    }
    let mut text = String::new();
    let mut timing = Vec::new();
    let mut untimed = Vec::new();
    let mut raw_parts = Vec::new();
    for (offset, frames, output) in parts {
        let part = output.track;
        if !text.is_empty() && !part.text.is_empty() {
            text.push('\n');
        }
        text.push_str(&part.text);
        let seconds = offset as f64 / 16_000.0;
        if options.review.is_some() && part.segments.is_empty() && !part.text.trim().is_empty() {
            untimed.push(part.text.clone());
        }
        let segment_values: Vec<_> = part
            .segments
            .iter()
            .map(|segment| {
                serde_json::json!({
                    "start": segment.start, "end": segment.end, "text": segment.text
                })
            })
            .collect();
        for segment in part.segments {
            timing.push(review::ReviewSegment {
                start: seconds + segment.start,
                end: seconds + segment.end,
                text: segment.text,
            });
        }
        let mut raw_part = serde_json::json!({
            "offset_frames": offset, "frames": frames,
            "sample_rate": 16000, "text": part.text, "segments": segment_values
        });
        if !output.speaker_segments.is_empty() {
            raw_part["speaker_segments"] =
                serde_json::to_value(output.speaker_segments).map_err(|e| e.to_string())?;
        }
        raw_parts.push(raw_part);
    }
    let id = request_key(
        &(
            input.to_string_lossy().as_ref(),
            &source_sha256,
            &transport::canonical_key(endpoint),
            &options.model,
            &options.language,
            options.timestamps,
            limit,
            &raw_parts,
        ),
        options.provider_profile,
    )?;
    let prefix = format!("{}-{}", safe_name(&input), &id[..16]);
    let mut manifest = serde_json::json!({
        "source": input, "source_sha256": source_sha256,
        "model": options.model, "sample_rate": 16000, "parts": raw_parts
    });
    if let Some(profile) = options.provider_profile {
        manifest["provider_profile"] = serde_json::to_value(profile).map_err(|e| e.to_string())?;
        manifest["endpoint"] = serde_json::Value::String(transport::canonical_key(endpoint));
    }
    let manifest = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    let (text_path, json_path) =
        publish_aggregate(&options.output_dir, &prefix, text.as_bytes(), &manifest)?;
    let saved = Aggregate {
        identity: aggregate_key,
        text: text_path.clone(),
        json: json_path,
    };
    let temp = aggregate_state.with_extension(format!("aggregate-{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp)
        .map_err(|e| format!("Cannot write aggregate state: {e}"))?;
    serde_json::to_writer(&mut file, &saved).map_err(|e| e.to_string())?;
    file.flush()
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(temp, aggregate_state).map_err(|e| format!("Cannot save aggregate state: {e}"))?;
    println!("{} -> {}", input.display(), text_path.display());
    Ok(review::ReviewTrack {
        label: safe_name(&input),
        audio: input,
        text,
        segments: timing,
        untimed,
    })
}

fn process_part(
    input: &Path,
    source_sha256: &str,
    upload: &Path,
    part: Option<(u64, u64)>,
    options: &Options,
    endpoint: &transport::Endpoint,
    key: Option<&str>,
    job_dir: &Path,
) -> Result<PartOutput, String> {
    let part_sha256 = if part.is_some() {
        digest(File::open(upload).map_err(|e| e.to_string())?)?
    } else {
        source_sha256.to_owned()
    };
    let id = request_key(
        &(
            input.to_string_lossy().as_ref(),
            source_sha256,
            &part_sha256,
            part,
            &transport::canonical_key(endpoint),
            &options.model,
            &options.language,
            options.timestamps,
        ),
        options.provider_profile,
    )?;
    let state_path = job_dir.join(format!("{id}.json"));
    let response_path = job_dir.join(format!("{id}.response.json"));
    let base = format!("{}-{}", safe_name(input), &id[..16]);
    let _lock = lock(&job_dir.join(format!("{id}.lock")))?;
    let existing = if state_path.exists() {
        Some(
            read_job_json::<Job>(File::open(&state_path).map_err(|e| e.to_string())?)
                .map_err(|e| format!("Cannot read existing job state: {e}"))?,
        )
    } else {
        None
    };
    if existing.is_none() && response_path.exists() {
        return Err("A saved response has no matching job state. Keep it for inspection and use a new output directory.".into());
    }
    if let Some(job) = &existing {
        if job.version != 1
            || job.identity != id
            || !matches!(job.status.as_str(), "unknown" | "failed" | "complete")
            || job.input != input
            || job.source_sha256 != source_sha256
            || job.endpoint != transport::canonical_key(endpoint)
            || job.model != options.model
            || job.language != options.language
            || job.timestamps != options.timestamps
            || job.provider_profile != options.provider_profile
            || job.chunk_offset_frames != part.map(|(offset, _)| offset)
            || job.chunk_frames != part.map(|(_, frames)| frames)
            || !valid_output_path(&job.text, &options.output_dir, &base, ".txt")
            || job
                .json
                .as_ref()
                .is_some_and(|path| !valid_output_path(path, &options.output_dir, &base, ".json"))
        {
            return Err("Job state is inconsistent or uses an unsupported version. Keep it for inspection and use a new output directory.".into());
        }
        if job.status == "complete" {
            let text = fs::read_to_string(&job.text)
                .map_err(|e| format!("Completed output is unavailable: {e}"))?;
            let raw = match &job.json {
                Some(path) => Some(
                    read_job_json(
                        File::open(path)
                            .map_err(|e| format!("Completed JSON is unavailable: {e}"))?,
                    )
                    .map_err(|e| format!("Completed JSON is invalid: {e}"))?,
                ),
                None => None,
            };
            println!(
                "{} -> {} (already complete)",
                input.display(),
                job.text.display()
            );
            return Ok(part_output(
                input,
                upload,
                part,
                options,
                text,
                raw.as_ref(),
            ));
        }
        if job.status == "unknown" && !response_path.exists() && !options.retry_unknown {
            return Err(
                "A previous request may have completed remotely. Use --retry-unknown to resend it."
                    .into(),
            );
        }
    }
    let mut prefix = base.clone();
    let mut attempt = 2u32;
    while options.output_dir.join(format!("{prefix}.txt")).exists()
        || options.output_dir.join(format!("{prefix}.json")).exists()
    {
        prefix = format!("{base}-{attempt}");
        attempt = attempt
            .checked_add(1)
            .ok_or("Too many existing output files")?;
    }
    let mut job = Job {
        version: 1,
        identity: id.clone(),
        status: "unknown".into(),
        input: input.to_owned(),
        source_sha256: source_sha256.to_owned(),
        endpoint: transport::canonical_key(endpoint),
        model: options.model.clone(),
        language: options.language.clone(),
        timestamps: options.timestamps,
        provider_profile: options.provider_profile,
        text: options.output_dir.join(format!("{prefix}.txt")),
        json: None,
        chunk_offset_frames: part.map(|(offset, _)| offset),
        chunk_frames: part.map(|(_, frames)| frames),
    };
    // Persist the ambiguity before sending any bytes. A crash cannot trigger an
    // implicit duplicate upload on the next invocation.
    save_state(&state_path, &job)?;
    let response = if response_path.exists() {
        // A response received before interruption is recoverable without another upload.
        let saved: CachedResponse =
            read_job_json(File::open(&response_path).map_err(|e| e.to_string())?)
                .map_err(|e| {
                    format!("Saved response is incomplete. Keep it for inspection and use a new output directory to retry: {e}")
                })?;
        transport::Response {
            content_type: saved.content_type,
            body: saved.body,
        }
    } else {
        let received = match transport::transcribe(
            endpoint,
            &options.model,
            key,
            options.language.as_deref(),
            upload,
            options.timestamps,
            options.provider_profile,
        ) {
            Ok(response) => response,
            Err(error) => {
                if !error.may_have_been_processed {
                    job.status = "failed".into();
                    save_state(&state_path, &job)?;
                }
                return Err(error.message);
            }
        };
        // Validate before saving, then sync the response before creating output.
        // This lets an interrupted local write resume without resending audio.
        parse_response(&received)?;
        let cache = CachedResponse {
            content_type: received.content_type.clone(),
            body: received.body.clone(),
        };
        let serialized = serde_json::to_vec(&cache).map_err(|e| e.to_string())?;
        publish_new(&response_path, &serialized)?;
        received
    };
    let (text, raw) = parse_response(&response)?;
    if raw.is_some() {
        job.json = Some(options.output_dir.join(format!("{prefix}.json")));
    }
    // Never overwrite prior output. A partial local write is left for inspection
    // and the job remains unknown until the user chooses an explicit retry.
    write_new(&job.text, text.as_bytes())?;
    if let (Some(path), Some(_)) = (&job.json, &raw) {
        write_new(path, &response.body)?;
    }
    job.status = "complete".into();
    save_state(&state_path, &job)?;
    println!("{} -> {}", input.display(), job.text.display());
    Ok(part_output(
        input,
        upload,
        part,
        options,
        text,
        raw.as_ref(),
    ))
}

fn part_output(
    input: &Path,
    upload: &Path,
    part: Option<(u64, u64)>,
    options: &Options,
    text: String,
    raw: Option<&serde_json::Value>,
) -> PartOutput {
    let speaker_segments = if options.provider_profile.is_some() {
        part.map(|(_, frames)| speaker_segments(raw, frames as f64 / 16_000.0))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    PartOutput {
        track: review::ReviewTrack {
            label: safe_name(input),
            audio: input.to_owned(),
            text,
            segments: review_segments(options, raw, upload, part),
            untimed: Vec::new(),
        },
        speaker_segments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversized_input_without_reading_locked_contents() {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{LockFile, UnlockFile};

        let folder = std::env::temp_dir().join(format!(
            "audio-capture-size-test-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&folder).unwrap();
        let path = folder.join("oversized.wav");
        File::create(&path)
            .unwrap()
            .set_len(1024 * 1024 + 1)
            .unwrap();
        let locked = File::open(&path).unwrap();
        let handle = locked.as_raw_handle();
        assert_ne!(unsafe { LockFile(handle, 0, 0, 1, 0) }, 0);

        let endpoint = transport::endpoint("http://127.0.0.1:1/v1", false).unwrap();
        let options = Options {
            max_file_mib: 1,
            ..Options::default()
        };
        let error = process_file(&path, &options, &endpoint, None, &folder).err();

        assert_ne!(unsafe { UnlockFile(handle, 0, 0, 1, 0) }, 0);
        drop(locked);
        fs::remove_file(path).unwrap();
        fs::remove_dir(folder).unwrap();
        assert!(
            error
                .as_ref()
                .is_some_and(|error| error.starts_with("Input exceeds the configured file limit")),
            "expected size rejection, got {error:?}"
        );
    }

    #[test]
    fn cached_response_json_uses_batched_reads() {
        struct CountedReader<'a> {
            contents: &'a [u8],
            calls: usize,
        }

        impl Read for CountedReader<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.calls += 1;
                self.contents.read(buffer)
            }
        }

        let body = vec![b'a'; 4096];
        let encoded = serde_json::to_vec(&CachedResponse {
            content_type: "text/plain".into(),
            body: body.clone(),
        })
        .unwrap();
        let mut input = CountedReader {
            contents: &encoded,
            calls: 0,
        };
        let saved: CachedResponse = read_job_json(&mut input).unwrap();

        assert_eq!(saved.content_type, "text/plain");
        assert_eq!(saved.body, body);
        assert!(
            input.calls <= 3,
            "expected batched reads, got {}",
            input.calls
        );
    }

    #[test]
    fn default_limit_is_25_mib() {
        assert_eq!(Options::default().max_file_mib, 25);
    }

    #[test]
    fn staged_response_never_replaces_a_saved_result() {
        let folder = std::env::temp_dir().join(format!(
            "audio-capture-job-test-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&folder).unwrap();
        let path = folder.join("response.json");
        publish_new(&path, b"first").unwrap();
        assert!(publish_new(&path, b"second").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"first");
        fs::remove_file(path).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn aggregate_recovers_when_previous_output_is_partial() {
        let folder = std::env::temp_dir().join(format!(
            "audio-capture-aggregate-test-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&folder).unwrap();
        let partial = folder.join("sample-combined.txt");
        fs::write(&partial, b"partial").unwrap();
        let (text, json) = publish_aggregate(&folder, "sample", b"complete", b"{}").unwrap();
        assert_eq!(fs::read(&partial).unwrap(), b"partial");
        assert_eq!(fs::read(&text).unwrap(), b"complete");
        assert_eq!(fs::read(&json).unwrap(), b"{}");
        let repeated = publish_aggregate(&folder, "sample", b"complete", b"{}").unwrap();
        assert_eq!(repeated, (text.clone(), json.clone()));
        let error = stage_and_publish_new(&text, b"overwrite").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&text).unwrap(), b"complete");
        fs::remove_file(partial).unwrap();
        fs::remove_file(text).unwrap();
        fs::remove_file(json).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn stale_staging_file_is_preserved_and_skipped() {
        let folder = std::env::temp_dir().join(format!(
            "audio-capture-stage-test-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&folder).unwrap();
        let path = folder.join("response.json");
        let stale = path.with_extension(format!("pending-{}-7", std::process::id()));
        fs::write(&stale, b"previous process").unwrap();
        let mut sequence = 7;
        stage_and_publish_with_sequence(&path, b"response", || {
            let next = sequence;
            sequence += 1;
            next
        })
        .unwrap();
        assert_eq!(fs::read(&stale).unwrap(), b"previous process");
        assert_eq!(fs::read(&path).unwrap(), b"response");
        fs::remove_file(stale).unwrap();
        fs::remove_file(path).unwrap();
        fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn output_paths_must_be_inside_the_job_directory() {
        let folder = std::env::temp_dir();
        assert!(valid_output_path(
            &folder.join("track-abc.txt"),
            &folder,
            "track-abc",
            ".txt"
        ));
        assert!(!valid_output_path(
            &folder.join("elsewhere").join("track-abc.txt"),
            &folder,
            "track-abc",
            ".txt"
        ));
        assert!(!valid_output_path(
            &folder.join("other.txt"),
            &folder,
            "track-abc",
            ".txt"
        ));
    }

    #[test]
    fn blocks_duplicate_tracks_unless_explicitly_selected() {
        let mut options = Options::default();
        options.files = vec![
            "Combined-1000000000000000000.opus".into(),
            "Example.exe-42-1000000000000000000.opus".into(),
        ];
        assert!(validate_batch(&options).is_err());
        options.allow_duplicate_sources = true;
        options.review = Some("review.html".into());
        assert!(validate_batch(&options).is_ok());
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            digest("abc".as_bytes()).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn provider_profile_keeps_legacy_job_keys_and_validates_modes() {
        let legacy = (
            "media",
            "digest",
            "http://localhost:8000/v1",
            "model",
            false,
        );
        let legacy_bytes = serde_json::to_vec(&legacy).unwrap();
        let profile = transport::ProviderProfile::OpenAiDiarizedV1;
        assert_eq!(
            request_key(&legacy, None).unwrap(),
            digest(legacy_bytes.as_slice()).unwrap()
        );
        assert_ne!(
            request_key(&legacy, None).unwrap(),
            request_key(&legacy, Some(profile)).unwrap()
        );
        let options = Options {
            provider_profile: Some(profile),
            timestamps: true,
            ..Options::default()
        };
        assert!(validate_batch(&options).is_err());
        assert!(
            "unrecognized"
                .parse::<transport::ProviderProfile>()
                .is_err()
        );
        assert_eq!("openai-diarized-v1".parse(), Ok(profile));
    }

    #[test]
    fn diarized_chunks_keep_valid_speaker_intervals_and_full_text() {
        let raw = serde_json::json!({
            "text": "one two three four",
            "segments": [
                {"start": 0.0, "end": 0.4, "text":"one","speaker":"A"},
                {"start": 0.3, "end": 0.5, "text":"two","speaker":"B"},
                {"start": 0.5, "end": 0.7, "text":"three","speaker":"\r"},
                {"start": 0.7, "end": 1.2, "text":"four","speaker":"C"}
            ],
        });
        let path = Path::new("example.wav");
        let options = Options {
            provider_profile: Some(transport::ProviderProfile::OpenAiDiarizedV1),
            ..Options::default()
        };
        let output = part_output(
            path,
            path,
            Some((32_000, 16_000)),
            &options,
            "one two three four".into(),
            Some(&raw),
        );
        assert_eq!(output.track.text, "one two three four");
        assert_eq!(output.speaker_segments.len(), 4);
        assert_eq!(output.speaker_segments[0].speaker.as_deref(), Some("A"));
        assert_eq!(output.speaker_segments[1].speaker.as_deref(), Some("B"));
        assert_eq!(output.speaker_segments[1].start, Some(0.3));
        assert_eq!(output.speaker_segments[1].end, Some(0.5));
        assert_eq!(output.speaker_segments[2].speaker, None);
        assert_eq!(output.speaker_segments[2].start, Some(0.5));
        assert_eq!(output.speaker_segments[3].speaker, None);
        assert_eq!(output.speaker_segments[3].start, None);
        let output = part_output(
            path,
            path,
            Some((0, 16_000)),
            &options,
            "text only".into(),
            None,
        );
        assert_eq!(output.track.text, "text only");
        assert!(output.speaker_segments.is_empty());
    }

    #[test]
    fn rejects_scaled_or_invalid_timestamps() {
        let value = serde_json::json!({"text":"a","segments":[{"start":0.0,"end":9640000000.0,"text":"a"}]});
        assert!(segments(Some(&value), Some(10.0)).is_empty());
        let value =
            serde_json::json!({"text":"a","segments":[{"start":0.0,"end":9.64,"text":"a"}]});
        assert_eq!(segments(Some(&value), Some(10.0)).len(), 1);
        assert!(segments(Some(&value), None).is_empty());
    }

    #[test]
    fn response_requires_text_and_supported_type() {
        let bad = transport::Response {
            content_type: "text/html".into(),
            body: b"error".to_vec(),
        };
        assert!(parse_response(&bad).is_err());
        let json = transport::Response {
            content_type: "application/json".into(),
            body: br#"{"text":""}"#.to_vec(),
        };
        assert_eq!(parse_response(&json).unwrap().0, "");
    }
}
