//! Durable, explicitly approved recorder batches. Standalone scripted jobs
//! continue to use the original job engine directly.
mod http;
pub(super) use http::transfer;
mod scheduler;
mod selection;

use crate::{attribution, guided, jobs, review, transport};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

pub(crate) use scheduler::reserve_live;
pub(super) use selection::expand;

fn id(prefix: &str) -> Result<String, String> {
    attribution::model::new_id(prefix)
}

fn state_root() -> Result<PathBuf, String> {
    let root = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is missing")?;
    Ok(PathBuf::from(root)
        .join("AudioCaptureRust")
        .join("batch-queue"))
}

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    serde_json::from_reader(BufReader::new(file)).map_err(|e| e.to_string())
}

pub(super) fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let tmp = path.with_extension(id("tmp")?);
    let write = || -> io::Result<()> {
        let mut f = File::create_new(&tmp)?;
        serde_json::to_writer(&mut f, value)?;
        f.flush()?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    };
    if let Err(err) = write() {
        let _ = fs::remove_file(&tmp);
        return Err(err.to_string());
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Input {
    path: PathBuf,
    digest: Option<String>,
    issue: Option<String>,
}

pub(super) fn snapshot(options: &jobs::Options) -> Vec<Input> {
    options
        .files
        .iter()
        .map(|path| {
            let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            let inspect = || -> Result<String, String> {
                if !jobs::is_media(&path) {
                    return Err("Choose a completed media file".into());
                }
                let file = OpenOptions::new()
                    .read(true)
                    .share_mode(1)
                    .open(&path)
                    .map_err(|e| format!("Cannot open a finished, stable recording: {e}"))?;
                let metadata = file.metadata().map_err(|e| e.to_string())?;
                if !metadata.is_file() || metadata.len() == 0 {
                    return Err("Expected a nonempty recording".into());
                }
                if !audio_capture::is_native_video(&path)
                    && !options.chunk_long
                    && metadata.len() > options.max_file_mib.saturating_mul(1024 * 1024)
                {
                    return Err("File exceeds the confirmed upload limit".into());
                }
                jobs::digest(file)
            };
            let (digest, issue) = match inspect() {
                Ok(h) => (Some(h), None),
                Err(e) => (None, Some(e)),
            };
            Input {
                path,
                digest,
                issue,
            }
        })
        .collect()
}

pub(super) fn print_issues(entries: &[Input], output: &mut impl Write) -> io::Result<()> {
    for entry in entries {
        if let Some(reason) = &entry.issue {
            writeln!(output, "Skip {}: {reason}", entry.path.display())?
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_container_size_does_not_reject_queue_approval() {
        let directory = std::env::temp_dir().join(id("queue_video_limit").unwrap());
        fs::create_dir(&directory).unwrap();
        let video_path = directory.join("selected.MP4");
        let audio_path = directory.join("selected.wav");
        for path in [&video_path, &audio_path] {
            File::create(path)
                .unwrap()
                .set_len(1024 * 1024 + 1)
                .unwrap();
        }
        let entries = snapshot(&jobs::Options {
            max_file_mib: 1,
            files: vec![video_path.clone(), audio_path.clone()],
            ..Default::default()
        });
        fs::remove_file(video_path).unwrap();
        fs::remove_file(audio_path).unwrap();
        fs::remove_dir(directory).unwrap();

        assert!(entries[0].digest.is_some() && entries[0].issue.is_none());
        assert!(entries[1].digest.is_none());
        assert_eq!(
            entries[1].issue.as_deref(),
            Some("File exceeds the confirmed upload limit")
        );
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum State {
    Pending,
    Active,
    Completed,
    Failed,
    Unknown,
    Cancelled,
}

#[derive(Serialize, Deserialize)]
struct Item {
    input: Input,
    state: State,
    detail: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Batch {
    version: u8,
    options: jobs::Options,
    items: Vec<Item>,
    #[serde(default)]
    speaker: Option<SpeakerJob>,
}

#[derive(Clone)]
pub(crate) struct ItemContext {
    dir: PathBuf,
    dispatch_marker: PathBuf,
    input_hash: String,
    cache_only: bool,
    unknown: Arc<AtomicBool>,
}

impl ItemContext {
    fn new(dir: &Path, index: usize, input_hash: &str, cache_only: bool) -> Self {
        Self {
            dir: dir.to_owned(),
            dispatch_marker: dir.join("items").join(format!("{index}.dispatched")),
            input_hash: input_hash.into(),
            cache_only,
            unknown: Arc::default(),
        }
    }
    fn check_control(&self) -> Result<(), String> {
        match control(&self.dir)?.as_str() {
            "run" => Ok(()),
            "pause" => Err("Batch paused".into()),
            "cancel" => Err("Batch cancelled".into()),
            _ => Err("Invalid batch control state".into()),
        }
    }
    pub(super) fn validate_input(&self, digest: &str) -> Result<(), String> {
        self.check_control()?;
        if self.input_hash != digest {
            Err("Input changed after confirmation. Select and confirm its current content.".into())
        } else {
            Ok(())
        }
    }
    pub(super) fn mark_unknown(&self) -> Result<(), String> {
        self.unknown.store(true, Ordering::Relaxed);
        match File::create_new(&self.dispatch_marker) {
            Ok(f) => f.sync_all().map_err(|e| e.to_string()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
    pub(super) fn clear_unknown(&self) {
        self.unknown.store(false, Ordering::Relaxed);
        let _ = fs::remove_file(&self.dispatch_marker);
    }
    pub(super) fn audio_staging(&self, bytes: u64) -> Result<File, String> {
        if bytes > 25 * 1024 * 1024 {
            return Err("A coordinated batch can stage up to 25 MiB of temporary audio.".into());
        }
        self.wait_for_lock(&state_root()?.join("audio-staging.lock"))
    }
    pub(super) fn wait_for_lock(&self, path: &Path) -> Result<File, String> {
        loop {
            self.check_control()?;
            if let Some(lock) = scheduler::exclusive(path).map_err(|e| e.to_string())? {
                return Ok(lock);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    pub(super) fn admit(
        &self,
        destination: &str,
    ) -> Result<(scheduler::Admission, Instant), String> {
        self.check_control()?;
        if self.cache_only {
            return Err("Completed review data is unavailable locally".into());
        }
        scheduler::reserve_request(destination, self)
    }
}

fn gui_owner() -> Result<Option<&'static OwnedHandle>, String> {
    static OWNER: OnceLock<Result<Option<OwnedHandle>, String>> = OnceLock::new();
    OWNER
        .get_or_init(|| {
            let Some(value) = std::env::var_os("AUDIOCAPTURE_GUI_PID") else {
                return Ok(None);
            };
            let pid = value
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or("Invalid recorder process ID")?;
            let handle = unsafe {
                windows_sys::Win32::System::Threading::OpenProcess(
                    windows_sys::Win32::System::Threading::SYNCHRONIZATION_SYNCHRONIZE,
                    0,
                    pid,
                )
            };
            if handle.is_null() {
                return Err("The recorder closed. Resume the batch explicitly.".into());
            }
            Ok(Some(unsafe { OwnedHandle::from_raw_handle(handle) }))
        })
        .as_ref()
        .map(|h| h.as_ref())
        .map_err(Clone::clone)
}

pub(super) fn bind_gui() -> Result<(), String> {
    gui_owner().map(|_| ())
}
fn gui_closed() -> Result<bool, String> {
    Ok(gui_owner()?.is_some_and(|owner|
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(owner.as_raw_handle(), 0)
        } != windows_sys::Win32::Foundation::WAIT_TIMEOUT))
}
fn control(dir: &Path) -> Result<String, String> {
    if gui_closed()? {
        return Ok("pause".into());
    }
    read_json(&dir.join("control.json"))
}

#[derive(Serialize, Deserialize)]
struct Progress {
    state: State,
    detail: Option<String>,
}
fn save_item(dir: &Path, index: usize, item: &Item) -> Result<(), String> {
    atomic_json(
        &dir.join("items").join(format!("{index}.json")),
        &Progress {
            state: item.state,
            detail: item.detail.clone(),
        },
    )
}
fn state_path(dir: &Path) -> PathBuf {
    dir.join("batch.json")
}
fn is_batch_id(value: &str) -> bool {
    value
        .strip_prefix("batch_")
        .is_some_and(|s| s.len() == 32 && s.bytes().all(|c| c.is_ascii_hexdigit()))
}
fn batch_dir(value: &str) -> Result<PathBuf, String> {
    if !is_batch_id(value) {
        return Err("Invalid batch ID. Run batch-queue list.".into());
    }
    let dir = state_root()?.join(value);
    if !state_path(&dir).is_file() {
        return Err("Batch does not exist".into());
    }
    Ok(dir)
}

pub(super) fn submit(
    mut options: jobs::Options,
    selected: Vec<Input>,
    speaker: Option<SpeakerJob>,
) -> Result<(), String> {
    jobs::validate_batch(&options)?;
    if !selected.iter().any(|i| i.digest.is_some()) {
        return Err("No eligible confirmed recordings".into());
    }
    options.output_dir = std::path::absolute(&options.output_dir).map_err(|e| e.to_string())?;
    options.review = options
        .review
        .as_ref()
        .map(std::path::absolute)
        .transpose()
        .map_err(|e| e.to_string())?;
    options.files = selected.iter().map(|i| i.path.clone()).collect();
    let root = state_root()?;
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let name = id("batch")?;
    let dir = root.join(&name);
    fs::create_dir(&dir).map_err(|e| e.to_string())?;
    fs::create_dir(dir.join("items")).map_err(|e| e.to_string())?;
    let items = selected
        .into_iter()
        .map(|input| Item {
            state: if input.digest.is_some() {
                State::Pending
            } else {
                State::Failed
            },
            detail: input.issue.clone(),
            input,
        })
        .collect();
    let batch = Batch {
        version: 1,
        options,
        items,
        speaker,
    };
    atomic_json(&dir.join("control.json"), &"run")?;
    atomic_json(&state_path(&dir), &batch)?;
    println!("Batch ID: {name}");
    drive(&dir, false, false)
}

fn effective_state(state: State, action: &str, owned: bool) -> &'static str {
    match state {
        State::Pending if action == "cancel" => "cancelled",
        State::Pending if action == "pause" => "paused",
        State::Pending if !owned => "awaiting resume",
        State::Active if !owned => "interrupted",
        State::Pending => "pending",
        State::Active => "active",
        State::Completed => "completed",
        State::Failed => "failed",
        State::Unknown => "unknown",
        State::Cancelled => "cancelled",
    }
}

fn load(dir: &Path) -> Result<Batch, String> {
    let mut batch: Batch = read_json(&state_path(dir))?;
    if batch.version != 1 {
        return Err("Unsupported batch version".into());
    }
    let progress = dir.join("items");
    if progress.is_dir() {
        for entry in fs::read_dir(&progress).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let index = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or("Invalid batch item index")?;
            let item = batch
                .items
                .get_mut(index)
                .ok_or("Batch item index is outside the plan")?;
            let saved: Progress = read_json(&path)?;
            item.state = saved.state;
            item.detail = saved.detail;
        }
    }
    if dir.join("speaker.json").is_file() {
        batch.speaker = Some(read_json(&dir.join("speaker.json"))?);
    }
    // Explicit retry consent applies only to the current invocation.
    batch.options.retry_unknown = false;
    Ok(batch)
}

fn drive(dir: &Path, resume: bool, retry_unknown: bool) -> Result<(), String> {
    let _driver = scheduler::exclusive(&dir.join("driver.lock"))
        .map_err(|e| e.to_string())?
        .ok_or("Another worker still owns this batch")?;
    let mut batch = load(dir)?;
    if resume {
        atomic_json(&dir.join("control.json"), &"run")?
    }
    let endpoint = transport::endpoint(&batch.options.base_url, batch.options.allow_http)?;
    let key = batch
        .options
        .key_env
        .as_ref()
        .map(|name| {
            std::env::var(name).map_err(|_| "The named API key environment variable is missing")
        })
        .transpose()?;
    if key.as_ref().is_some_and(String::is_empty) {
        return Err("The named API key environment variable is empty".into());
    }
    fs::create_dir_all(&batch.options.output_dir).map_err(|e| e.to_string())?;
    let mut opts = batch.options.clone();
    opts.output_dir = opts.output_dir.canonicalize().map_err(|e| e.to_string())?;
    let job_dir = opts.output_dir.join(".audio-capture-jobs");
    fs::create_dir_all(&job_dir).map_err(|e| e.to_string())?;
    let need_review = opts.review.as_ref().is_some_and(|p| !p.exists());
    let mut tracks = Vec::new();
    for index in 0..batch.items.len() {
        let action = control(dir)?;
        if action != "run" {
            break;
        }
        let item = &batch.items[index];
        let Some(digest) = item.input.digest.clone() else {
            continue;
        };
        let done = item.state == State::Completed;
        if done && !need_review {
            continue;
        }
        let path = item.input.path.clone();
        let context = ItemContext::new(dir, index, &digest, done);
        opts.coordinated = Some(context.clone());
        opts.retry_unknown = retry_unknown;
        if !done {
            batch.items[index].state = State::Active;
            batch.items[index].detail = None;
            save_item(dir, index, &batch.items[index])?;
        }
        let result = jobs::process_file(&path, &opts, &endpoint, key.as_deref(), &job_dir);
        let action = control(dir)?;
        let item = &mut batch.items[index];
        match result {
            Ok(track) => {
                item.state = State::Completed;
                item.detail = None;
                if need_review {
                    tracks.push(track)
                }
            }
            Err(err) => {
                item.state = if done {
                    State::Completed
                } else if context.unknown.load(Ordering::Relaxed) {
                    State::Unknown
                } else if action == "pause" {
                    State::Pending
                } else if action == "cancel" {
                    State::Cancelled
                } else {
                    State::Failed
                };
                eprintln!("{}: {err}", path.display());
                item.detail = Some(err);
            }
        }
        save_item(dir, index, &batch.items[index])?;
    }
    let action = control(dir)?;
    if action != "run" {
        println!("Batch {action} requested. Completed transcripts remain available.");
        return Ok(());
    }
    if let Some(path) = opts
        .review
        .as_ref()
        .filter(|_| need_review && !tracks.is_empty())
    {
        jobs::disambiguate_labels(&mut tracks);
        review::write_review(path, &tracks, opts.same_session).map_err(|e| e.to_string())?;
        println!("Open the local review: {}", path.display());
    }
    let completed_audio: Vec<_> = batch
        .items
        .iter()
        .filter(|i| i.state == State::Completed)
        .map(|i| i.input.clone())
        .collect();
    if let Some(job) = batch.speaker.as_mut().filter(|j| !j.complete) {
        let result = run_speakers(job, &completed_audio);
        job.complete = result.is_ok();
        job.error = result.as_ref().err().cloned();
        atomic_json(&dir.join("speaker.json"), &job)?;
        if let Err(e) = result {
            eprintln!("Local speaker analysis: {e}");
            return Err("Speaker analysis failed. Completed transcripts remain available.".into());
        }
    }
    let failures = batch
        .items
        .iter()
        .filter(|i| i.state != State::Completed)
        .count();
    if failures > 0 {
        Err(format!(
            "{failures} item(s) failed or need an explicit retry"
        ))
    } else {
        println!("Transcription complete.");
        Ok(())
    }
}

fn list() -> Result<(), String> {
    let root = state_root()?;
    if !root.is_dir() {
        println!("No saved batches.");
        return Ok(());
    }
    let mut ids = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let e = entry.map_err(|e| e.to_string())?;
        let n = e.file_name();
        if let Some(n) = n.to_str().filter(|n| is_batch_id(n)) {
            ids.push(n.to_owned())
        }
    }
    ids.sort();
    if ids.is_empty() {
        println!("No saved batches.")
    }
    for name in ids {
        let dir = batch_dir(&name)?;
        let b = load(&dir)?;
        println!(
            "{name}  {} / {} completed",
            b.items
                .iter()
                .filter(|i| i.state == State::Completed)
                .count(),
            b.items.len()
        );
    }
    Ok(())
}

fn status(dir: &Path) -> Result<(), String> {
    let batch = load(dir)?;
    let action = control(dir)?;
    let owned = scheduler::exclusive(&dir.join("driver.lock"))
        .map_err(|e| e.to_string())?
        .is_none();
    if let Some(job) = batch.speaker.as_ref() {
        println!(
            "Speaker analysis: {}",
            job.error
                .as_deref()
                .unwrap_or(if job.complete { "completed" } else { "pending" })
        );
    }
    for (index, item) in batch.items.into_iter().enumerate() {
        let interrupted = !owned
            && item.state == State::Active
            && dir
                .join("items")
                .join(format!("{index}.dispatched"))
                .is_file();
        let state = if interrupted {
            State::Unknown
        } else {
            item.state
        };
        println!(
            "{}  {}  {}",
            effective_state(state, &action, owned),
            item.input.path.display(),
            item.detail.unwrap_or_default()
        );
    }
    Ok(())
}

const HELP: &str = "Usage: transcribe batch-queue list | manage | status ID | pause ID | cancel ID | resume ID [--retry-unknown] | submit --attribute-session MANIFEST -- FILE... | attribute ID --session MANIFEST";
pub(crate) fn run(args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let args: Vec<_> = args.collect();
    if args.len() == 1 && args[0] == "--http-transfer" {
        return http::child();
    }
    bind_gui()?;
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!("{HELP}");
        return Ok(());
    }
    if args.len() == 1 && args[0] == "list" {
        return list();
    }
    if args.len() >= 5 && args[0] == "submit" && args[1] == "--attribute-session" && args[3] == "--"
    {
        return guided::run_with_speakers(args[3..].iter().cloned(), Some(Path::new(&args[2])));
    }
    if args.len() == 1 && args[0] == "manage" {
        list()?;
        println!(
            "Enter status, pause, cancel or resume followed by a batch ID. Press Enter to close."
        );
        let line = guided::read_answer(&mut io::stdin().lock()).map_err(|e| e.to_string())?;
        let words: Vec<_> = line.split_whitespace().collect();
        if words.is_empty() {
            return Ok(());
        }
        if !matches!(words[0], "status" | "pause" | "cancel" | "resume") {
            return Err(HELP.into());
        }
        return run(words
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
            .into_iter());
    }
    let Some(name) = args.get(1).and_then(|x| x.to_str()) else {
        return Err(HELP.into());
    };
    let dir = batch_dir(name)?;
    match args.first().and_then(|v| v.to_str()) {
        Some("status") if args.len() == 2 => status(&dir),
        Some(action @ ("pause" | "cancel")) if args.len() == 2 => {
            atomic_json(&dir.join("control.json"), &action)
        }
        Some("resume") if args.len() == 2 || (args.len() == 3 && args[2] == "--retry-unknown") => {
            drive(&dir, true, args.len() == 3)
        }
        Some("attribute") if args.len() == 4 && args[2] == "--session" => {
            attribute_selection(&dir, Path::new(&args[3]))
        }
        _ => Err(HELP.into()),
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct SpeakerJob {
    path: PathBuf,
    digest: String,
    complete: bool,
    error: Option<String>,
}

pub(super) fn preview_speaker(path: &Path, output: &mut impl Write) -> Result<SpeakerJob, String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let selected: attribution::model::Manifest =
        serde_json::from_reader(BufReader::new(&file)).map_err(|e| e.to_string())?;
    let adapter = selected.adapter.as_ref();
    writeln!(output, "Local speaker session: {}", path.display()).map_err(|e| e.to_string())?;
    if let Some(adapter) = adapter {
        writeln!(
            output,
            "Local speaker adapter: {:?} {:?}",
            adapter.program, adapter.args
        )
        .map_err(|e| e.to_string())?;
    } else {
        writeln!(
            output,
            "Local adapter missing. Recognition can still finish."
        )
        .map_err(|e| e.to_string())?;
    }
    for s in attribution::select_sources(
        &selected,
        path.parent().ok_or("Missing speaker manifest parent")?,
    )? {
        writeln!(output, "  Speaker input: {}", s.audio.display()).map_err(|e| e.to_string())?;
    }
    let digest = jobs::digest(File::open(&path).map_err(|e| e.to_string())?)?;
    Ok(SpeakerJob {
        path,
        digest,
        complete: false,
        error: None,
    })
}

fn run_speakers(job: &SpeakerJob, completed: &[Input]) -> Result<(), String> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&job.path)
        .map_err(|e| e.to_string())?;
    if jobs::digest(&file)? != job.digest {
        return Err("The approved speaker manifest changed. Confirm it again.".into());
    }
    let manifest: attribution::model::Manifest = read_json(&job.path)?;
    if manifest.adapter.is_none() {
        return Err("A local speaker adapter is required".into());
    }
    let parent = job.path.parent().ok_or("Missing speaker manifest parent")?;
    let mut held = Vec::new();
    let mut excluded = std::collections::BTreeMap::new();
    let approved: std::collections::HashMap<_, _> = completed
        .iter()
        .filter_map(|item| Some((item.path.canonicalize().ok()?, item.digest.as_deref()?)))
        .collect();
    for source in attribution::select_sources(&manifest, parent)? {
        let check = || -> Result<File, String> {
            let audio = parent
                .join(&source.audio)
                .canonicalize()
                .map_err(|e| e.to_string())?;
            let digest = approved
                .get(&audio)
                .ok_or("Outside the completed batch selection")?;
            let input = OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&audio)
                .map_err(|e| e.to_string())?;
            if jobs::digest(&input)? != *digest {
                return Err("The approved recording changed".into());
            }
            Ok(input)
        };
        match check() {
            Ok(input) => held.push(input),
            Err(reason) => {
                eprintln!("Skip local speaker input {}: {reason}", source.source_id);
                excluded.insert(source.source_id.clone(), reason);
            }
        }
    }
    let session = attribution::attribute_with_eligibility(&job.path, Some(&excluded))?;
    let problems: Vec<_> = session
        .sources
        .iter()
        .filter_map(|source| {
            let issue = source.provenance["adapter_error"].as_str().or_else(|| {
                (!matches!(source.status.as_str(), "success" | "silent" | "untimed"))
                    .then_some(source.status.as_str())
            })?;
            Some(format!("{}: {issue}", source.source_id))
        })
        .collect();
    if !session.complete || !problems.is_empty() {
        return Err(format!(
            "Partial speaker attribution. {}. Recognition results remain available.",
            problems.join(", ")
        ));
    }
    println!(
        "Completed speaker attribution, revision {}.",
        session.revision
    );
    Ok(())
}

fn attribute_selection(dir: &Path, path: &Path) -> Result<(), String> {
    let batch = load(dir)?;
    let job = preview_speaker(path, &mut io::stdout().lock())?;
    println!("Run local speaker attribution on these completed inputs? [y/N]");
    let answer = guided::read_answer(&mut io::stdin().lock()).map_err(|e| e.to_string())?;
    if !matches!(answer.to_ascii_lowercase().as_str(), "yes" | "y") {
        println!("Cancelled.");
        return Ok(());
    }
    run_speakers(
        &job,
        &batch
            .items
            .into_iter()
            .filter(|i| i.state == State::Completed)
            .map(|i| i.input)
            .collect::<Vec<_>>(),
    )
}
