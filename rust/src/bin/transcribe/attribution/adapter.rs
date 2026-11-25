//! One local diarizer per source. A Windows job owns the adapter and decoder.
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::ptr::null;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, GetProcessIdOfThread, OpenThread, ResumeThread,
    THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
};

const OUTPUT_LIMIT: usize = 4 << 20;
const DIAGNOSTIC_LIMIT: usize = 16 << 10;
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 300;
pub const MAX_TIMEOUT_SECONDS: u64 = 14_400;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterConfig {
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Interval {
    pub speaker: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterOutput {
    protocol_version: u32,
    source_id: String,
    pub backend: String,
    pub model: String,
    pub intervals: Vec<Interval>,
}

fn parse(bytes: &[u8], source_id: &str, duration_ms: u64) -> Result<AdapterOutput, String> {
    let mut result: AdapterOutput =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid adapter JSON: {e}"))?;
    let label = |s: &str| !s.trim().is_empty() && s.len() <= 512 && !s.contains(char::is_control);
    if result.protocol_version != 1 || result.source_id != source_id {
        return Err("Adapter version or source ID differs from the request.".into());
    }
    if !label(&result.backend) || !label(&result.model) {
        return Err("Adapter must identify its backend and model.".into());
    }
    for interval in &result.intervals {
        if !label(&interval.speaker)
            || interval.start_ms >= interval.end_ms
            || interval.end_ms > duration_ms
        {
            return Err("Adapter returned an invalid speaker or interval.".into());
        }
    }
    result.intervals.sort_by(|a, b| {
        (a.start_ms, a.end_ms, &a.speaker).cmp(&(b.start_ms, b.end_ms, &b.speaker))
    });
    result.intervals.dedup();
    Ok(result)
}

pub(crate) fn job() -> Result<OwnedHandle, String> {
    // The job handle is private and non-inheritable. Closing the last handle
    // terminates every member, even if the worker exits before normal cleanup.
    let raw = unsafe { CreateJobObjectW(null(), null()) };
    if raw.is_null() {
        return Err(format!(
            "Cannot create adapter job: {}",
            io::Error::last_os_error()
        ));
    }
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    limits.BasicLimitInformation.ActiveProcessLimit = 8;
    let ok = unsafe {
        SetInformationJobObject(
            raw,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of_val(&limits) as u32,
        )
    };
    if ok == 0 {
        return Err(format!(
            "Cannot set adapter limits: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(job)
}

pub(crate) fn start_in_job(child: &Child, job: &OwnedHandle) -> Result<(), String> {
    let ok = unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) };
    if ok == 0 {
        return Err(format!(
            "Cannot attach adapter to its job: {}",
            io::Error::last_os_error()
        ));
    }
    // Rust 1.99 does not expose Child's primary thread handle. The suspended
    // process has exactly one thread. Resume it only after assigning the job.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }
        .map_err(|e| format!("Cannot locate the adapter startup thread: {e}"))?;
    let _snapshot_owner = unsafe { OwnedHandle::from_raw_handle(snapshot.0) };
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut next = unsafe { Thread32First(snapshot, &mut entry) };
    while next.is_ok() {
        if entry.th32OwnerProcessID == child.id() {
            let raw = unsafe {
                OpenThread(
                    THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                    0,
                    entry.th32ThreadID,
                )
            };
            if raw.is_null() {
                break;
            }
            let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
            if unsafe { GetProcessIdOfThread(handle.as_raw_handle()) } == child.id()
                && unsafe { ResumeThread(handle.as_raw_handle()) } == 1
            {
                return Ok(());
            }
            break;
        }
        next = unsafe { Thread32Next(snapshot, &mut entry) };
    }
    Err("Cannot resume the adapter startup thread.".into())
}

fn read_limited(reader: impl Read, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("Adapter output exceeded its byte limit.".into());
    }
    Ok(bytes)
}

pub fn run(
    config: &AdapterConfig,
    source_id: &str,
    audio: &Path,
    duration_ms: u64,
) -> Result<AdapterOutput, String> {
    if !(1..=MAX_TIMEOUT_SECONDS).contains(&config.timeout_seconds) || duration_ms == 0 {
        return Err(
            "Supply a positive source duration and an adapter timeout of 1 to 14400 seconds."
                .into(),
        );
    }
    if !audio.is_file() {
        return Err("Adapter audio file is unavailable.".into());
    }
    let audio = std::path::absolute(audio).map_err(|e| e.to_string())?;
    let input = serde_json::to_vec(&serde_json::json!({
        "protocol_version": 1, "source_id": source_id,
        "audio_path": audio.to_str().ok_or("Adapter audio path is not valid Unicode.")?,
        "duration_ms": duration_ms,
    }))
    .map_err(|e| e.to_string())?;
    if input.len() > 65536 {
        return Err("Adapter request exceeded 64 KiB.".into());
    }
    let job = job()?;
    let mut child = Command::new(&config.program)
        .args(&config.args)
        .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Cannot start local diarizer: {e}"))?;
    if let Err(error) = start_in_job(&child, &job) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let deadline = Instant::now() + Duration::from_secs(config.timeout_seconds);

    // Read and write concurrently so a full pipe cannot defeat the deadline.
    // On error, close the job before joining the threads to release their pipes.
    enum Pipe {
        Exited(std::process::ExitStatus),
        Output(Vec<u8>),
        Diagnostic(Vec<u8>),
    }
    let output = thread::scope(|scope| -> Result<Vec<u8>, String> {
        let (tx, rx) = mpsc::channel();
        let writer = tx.clone();
        scope.spawn(move || {
            let sent = stdin.write_all(&input);
            drop(stdin);
            let result = child.wait().and_then(|exit| {
                if exit.success() {
                    sent?;
                }
                Ok(Pipe::Exited(exit))
            });
            let _ = writer.send(result.map_err(|e| e.to_string()));
        });
        let reader = tx.clone();
        scope.spawn(move || {
            let _ = reader.send(read_limited(stdout, OUTPUT_LIMIT).map(Pipe::Output));
        });
        scope.spawn(move || {
            let _ = tx.send(read_limited(stderr, DIAGNOSTIC_LIMIT).map(Pipe::Diagnostic));
        });

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut status = None;
        let mut guard = Some(job);
        for _ in 0..3 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let result = rx.recv_timeout(remaining).map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => "Local diarization timed out.",
                mpsc::RecvTimeoutError::Disconnected => "Adapter pipe closed unexpectedly.",
            })?;
            match result? {
                Pipe::Output(bytes) => stdout = bytes,
                Pipe::Diagnostic(bytes) => stderr = bytes,
                Pipe::Exited(exit) => {
                    status = Some(exit);
                    // A leftover descendant must not hold a pipe open after
                    // the adapter exits. Closing the job terminates it.
                    drop(guard.take());
                }
            }
        }
        let exit = status.ok_or("Missing adapter exit status.")?;
        if !exit.success() {
            return Err(format!(
                "Local diarizer exited with {exit}: {}",
                String::from_utf8_lossy(&stderr).trim()
            ));
        }
        Ok(stdout)
    });
    parse(&output?, source_id, duration_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply(intervals: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "protocol_version":1, "source_id":"track1", "backend":"test 1", "model":"m1",
            "intervals":intervals
        }))
        .unwrap()
    }

    #[test]
    fn validates_scope_and_intervals_but_preserves_overlapping_speakers() {
        let bytes = reply(json!([
            {"speaker":"B","start_ms":3,"end_ms":8}, {"speaker":"A","start_ms":1,"end_ms":5}
        ]));
        let parsed = parse(&bytes, "track1", 8).unwrap();
        assert_eq!(parsed.intervals[0].speaker, "A");
        assert_eq!(parsed.intervals.len(), 2);
        assert!(parse(&bytes, "another", 8).is_err());
        assert!(parse(&bytes, "track1", 7).is_err());
        for (start, end) in [(0, 0), (-1, 2), (3, 1)] {
            let invalid = reply(json!([{"speaker":"a","start_ms":start,"end_ms":end}]));
            assert!(parse(&invalid, "track1", 8).is_err());
        }
    }

    #[test]
    fn pipe_reader_caps_bytes_before_parsing() {
        assert!(read_limited(&b"12345"[..], 4).is_err());
        assert_eq!(read_limited(&b"1234"[..], 4).unwrap(), b"1234");
    }

    #[test]
    fn unresponsive_adapter_fixture() {
        // The nested test harness receives this marker as a --skip value.
        if std::env::args().any(|arg| arg == "simulate-adapter-stall") {
            thread::sleep(Duration::from_secs(30));
        }
    }

    #[test]
    fn deadline_stops_an_unresponsive_adapter() {
        let executable = std::env::current_exe().unwrap();
        let config = AdapterConfig {
            program: executable.clone(),
            args: vec![
                "unresponsive_adapter_fixture".into(),
                "--skip".into(),
                "simulate-adapter-stall".into(),
            ],
            timeout_seconds: 1,
        };
        let start = Instant::now();
        assert!(
            run(&config, "test", &executable, 1000)
                .unwrap_err()
                .contains("timed out")
        );
        assert!(start.elapsed() < Duration::from_secs(10));
    }
}
