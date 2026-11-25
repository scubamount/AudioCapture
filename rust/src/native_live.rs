//! Native-side lifetime of an explicitly approved transcription worker.
use std::io::{BufRead, BufReader, Read};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use audio_capture::transcription_config::validate_connection;
use serde::Deserialize;
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CreateEventW, SetEvent};

const MAX_LINE: usize = 64 * 1024;
const MAX_PREVIEW: usize = 2048;

#[derive(Clone)]
pub(crate) struct Options {
    pub endpoint: String,
    pub model: String,
    pub key_env: String,
    pub folder: PathBuf,
    pub sources: Vec<String>,
    pub gate: bool,
    pub timestamps: bool,
    pub speaker_config: Option<PathBuf>,
}

impl Options {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.sources.is_empty() {
            return Err("Select a program, microphone, or system audio first.".into());
        }
        if self.folder.as_os_str().is_empty() {
            return Err("Select an output folder for the transcript.".into());
        }
        if self.sources.len() > 16
            || self.sources.iter().enumerate().any(|(index, source)| {
                source.is_empty()
                    || source.len() > 256
                    || source.chars().any(char::is_control)
                    || self.sources[..index].contains(source)
            })
        {
            return Err(
                "Live transcription needs up to 16 distinct source names of 256 bytes or less."
                    .into(),
            );
        }
        validate_connection(&self.endpoint, &self.model, &self.key_env)?;
        if !self.key_env.is_empty()
            && std::env::var_os(&self.key_env).is_none_or(|value| value.is_empty())
        {
            return Err("The API key variable is missing or empty. Check the variable name in Transcription settings.".into());
        }
        Ok(())
    }

    pub(crate) fn confirmation(&self, scope: &str, speakers: bool) -> String {
        format!(
            "Sources: {scope}\nRecognition endpoint: {}\nModel: {}\nOutput folder: {}\nAPI key variable: {}\nSpeech gating: {}\nSegment timestamps: {}\nLocal speaker labels: {}\n\nSend live audio from these sources to this endpoint until you stop transcription? Pending chunks can finish after Stop. The worker saves readable text locally.",
            self.endpoint,
            self.model,
            self.folder.display(),
            if self.key_env.is_empty() {
                "None"
            } else {
                &self.key_env
            },
            if self.gate { "On" } else { "Off" },
            if self.timestamps { "On" } else { "Off" },
            if speakers { "On" } else { "Off" }
        )
    }
}

#[derive(Default, Clone)]
pub(crate) struct Status {
    pub ready: bool,
    pub state: String,
    pub message: String,
    pub path: Option<PathBuf>,
    pub preview: String,
}

#[derive(Deserialize)]
struct Update {
    state: String,
    #[serde(default)]
    message: String,
    path: Option<PathBuf>,
    preview: Option<String>,
}

fn update(status: &mut Status, line: &[u8]) {
    let Ok(value) = serde_json::from_slice::<Update>(line) else {
        return;
    };
    if value.state == "ready" {
        status.ready = true;
    }
    status.state = value.state;
    status.message = value.message.chars().take(1200).collect();
    if let Some(path) = value.path {
        status.path = Some(path);
    }
    if let Some(preview) = value.preview {
        let from = preview
            .char_indices()
            .rev()
            .nth(MAX_PREVIEW - 1)
            .map_or(0, |(index, _)| index);
        status.preview = preview[from..].to_owned();
    }
}

// Drain both pipes off the UI thread. A malformed worker line cannot grow memory indefinitely.
fn read_lines(reader: impl Read, mut handle: impl FnMut(&[u8])) {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    let mut oversized = false;
    while let Ok(buf) = reader.fill_buf() {
        if buf.is_empty() {
            break;
        }
        let end = buf.iter().position(|b| *b == b'\n');
        let count = end.unwrap_or(buf.len());
        if line.len().saturating_add(count) > MAX_LINE {
            oversized = true;
        }
        if !oversized {
            line.extend_from_slice(&buf[..count]);
        }
        reader.consume(count + usize::from(end.is_some()));
        if end.is_some() {
            if !oversized {
                handle(&line);
            }
            oversized = false;
            line.clear();
        }
    }
    if !oversized && !line.is_empty() {
        handle(&line);
    }
}

pub(crate) struct Worker {
    child: Child,
    cancel_event: OwnedHandle,
    stdin: Option<ChildStdin>,
    status: Arc<Mutex<Status>>,
    stderr: Arc<Mutex<String>>,
    readers: [JoinHandle<()>; 2],
    pub options: Options,
    pub interval: String,
    pub started: Option<Instant>,
}

impl Worker {
    pub(crate) fn spawn(options: Options) -> Result<Self, String> {
        options.validate()?;
        let worker = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name("transcribe.exe");
        let interval = audio_capture::live_wire::new_interval_id();
        let event_name: Vec<u16> = format!(r"Local\AudioCapture.Live.Cancel.{interval}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // The kernel owns the event until both processes close their handles.
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, event_name.as_ptr()) };
        if handle.is_null() {
            return Err(format!(
                "Cannot create a transcription cancel event: {}",
                std::io::Error::last_os_error()
            ));
        }
        let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let cancel_event = unsafe { OwnedHandle::from_raw_handle(handle) };
        if exists {
            return Err("A transcription interval with this ID already exists.".into());
        }
        let mut cmd = Command::new(worker);
        cmd.env("AUDIOCAPTURE_GUI_PID", std::process::id().to_string());
        cmd.args([
            "live",
            "--confirmed",
            "--base-url",
            &options.endpoint,
            "--model",
            &options.model,
            "--output-dir",
        ])
        .arg(&options.folder);
        cmd.args(["--cancel-event-id", &interval]);
        for source in &options.sources {
            cmd.arg("--source").arg(source);
        }
        if !options.key_env.is_empty() {
            cmd.args(["--api-key-env", &options.key_env]);
        }
        if options.gate {
            cmd.arg("--speech-gate");
        }
        if options.timestamps {
            cmd.arg("--timestamps");
        }
        if let Some(path) = &options.speaker_config {
            cmd.arg("--speaker-config").arg(path);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("Cannot start the companion transcribe.exe: {e}"))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let errors = child.stderr.take().expect("piped stderr");
        let status = Arc::new(Mutex::new(Status::default()));
        let stderr = Arc::new(Mutex::new(String::new()));
        let output_status = Arc::clone(&status);
        let output_errors = Arc::clone(&stderr);
        let status_reader = thread::Builder::new()
            .name("live-worker-status".into())
            .spawn(move || {
                read_lines(stdout, |line| {
                    update(&mut output_status.lock().unwrap(), line)
                })
            })
            .map_err(|e| {
                let _ = child.kill();
                format!("Cannot read transcription status: {e}")
            })?;
        let error_reader = thread::Builder::new()
            .name("live-worker-errors".into())
            .spawn(move || {
                read_lines(errors, |line| {
                    *output_errors.lock().unwrap() =
                        String::from_utf8_lossy(line).chars().take(1200).collect()
                })
            })
            .map_err(|e| {
                let _ = child.kill();
                format!("Cannot read transcription errors: {e}")
            })?;
        Ok(Self {
            child,
            cancel_event,
            stdin,
            status,
            stderr,
            readers: [status_reader, error_reader],
            options,
            interval,
            started: None,
        })
    }

    pub(crate) fn tap(&mut self) -> Result<Box<dyn audio_capture::record::LiveTap>, String> {
        let stdin = self
            .stdin
            .take()
            .ok_or("The worker input was already attached.")?;
        audio_capture::live_wire::launch_relay(
            stdin,
            self.interval.clone(),
            self.options.sources.clone(),
        )
        .map_err(|e| e.to_string())
    }

    pub(crate) fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub(crate) fn path(&self) -> Option<PathBuf> {
        self.status.lock().unwrap().path.clone()
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.status.lock().unwrap().ready
    }

    pub(crate) fn cancel_drain(&self) -> Result<(), String> {
        if unsafe { SetEvent(self.cancel_event.as_raw_handle()) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }

    pub(crate) fn cancel_wait(&mut self) {
        self.stdin.take();
        // No frames have been admitted. A worker blocked on a busy batch slot may not read EOF.
        let _ = self.child.kill();
    }

    pub(crate) fn result(&mut self) -> Option<Result<Status, String>> {
        let code = match self.child.try_wait() {
            Ok(Some(code)) => code,
            Ok(None) => return None,
            Err(error) => return Some(Err(error.to_string())),
        };
        if !self.readers.iter().all(JoinHandle::is_finished) {
            return None;
        }
        let status = self.status();
        if code.success()
            && matches!(
                status.state.as_str(),
                "complete" | "incomplete" | "cancelled"
            )
        {
            return Some(Ok(status));
        }
        let detail = self.stderr.lock().unwrap();
        let reason = if !detail.is_empty() {
            detail.as_str()
        } else if !status.message.is_empty() {
            &status.message
        } else {
            "The worker ended before it reported completion."
        };
        Some(Err(reason.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn example_options() -> Options {
        Options {
            endpoint: "http://127.0.0.1:9090/v1".into(),
            model: "fixture".into(),
            key_env: String::new(),
            folder: PathBuf::from("output"),
            sources: vec!["Program".into(), "Microphone".into()],
            gate: false,
            timestamps: false,
            speaker_config: None,
        }
    }
    #[test]
    fn consent_lists_only_selected_capabilities_and_supports_timestamps() {
        let mut options = example_options();
        options.validate().unwrap();
        let consent = options.confirmation("Program (PID 123) and Microphone: Test", false);
        assert!(consent.contains("Program (PID 123) and Microphone: Test"));
        assert!(consent.contains("Segment timestamps: Off"));
        assert!(consent.contains("Speech gating: Off"));
        assert!(consent.contains("API key variable: None"));
        options.timestamps = true;
        assert!(
            options
                .confirmation("Program", false)
                .contains("Segment timestamps: On")
        );
    }
    #[test]
    fn validation_rejects_embedded_credentials_and_invalid_scopes_before_consent() {
        let mut options = example_options();
        options.endpoint = "https://name:password@example.org".into();
        let error = options.validate().unwrap_err();
        assert!(!error.contains("password"));
        let mut options = example_options();
        options.key_env = "test-secret-token".into();
        assert!(!options.validate().unwrap_err().contains(&options.key_env));
        options.key_env.clear();
        options.sources.push("Program".into());
        assert!(options.validate().is_err());
    }
    #[test]
    fn status_preserves_cache_and_bounds_unicode_preview() {
        let mut status = Status::default();
        update(&mut status, br#"{"state":"ready","path":"partial.txt"}"#);
        let line = serde_json::json!({"state":"active", "preview":"中".repeat(10000)}).to_string();
        update(&mut status, line.as_bytes());
        assert!(status.ready);
        assert_eq!(status.path, Some(PathBuf::from("partial.txt")));
        assert_eq!(status.preview.chars().count(), MAX_PREVIEW);
    }
    #[test]
    fn oversized_status_does_not_hide_the_next_event() {
        let mut input = vec![b'x'; MAX_LINE + 1];
        input.extend_from_slice(b"\nlast\n");
        let mut seen = Vec::new();
        read_lines(&input[..], |line| seen.push(line.to_vec()));
        assert_eq!(seen, [b"last".to_vec()]);
    }
}
