//! Hidden recorder-to-worker transport. Only explicitly selected operations start a worker.
use audio_capture::transcription_config::{GUI_CONFIG_ENV, GUI_PID_ENV, TranscriptionConfig};
use serde_json::Value;
use std::{
    io::{Read, Write},
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    thread,
};
use windows_sys::Win32::System::Threading::DETACHED_PROCESS;

fn command() -> Result<Command, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe.with_file_name("transcribe.exe"));
    cmd.creation_flags(DETACHED_PROCESS)
        .env_remove(GUI_PID_ENV)
        .env_remove(GUI_CONFIG_ENV)
        .env_remove("AUDIO_CAPTURE_GRANT_REF")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    Ok(cmd)
}

pub(crate) fn control(
    group: &str,
    action: &str,
    grant: Option<&str>,
    input: Value,
) -> Result<Value, String> {
    let mut cmd = command()?;
    if let Some(grant) = grant {
        cmd.env("AUDIO_CAPTURE_GRANT_REF", grant);
    }
    let mut child = cmd
        .args(["control", group, action, "--json", "--input", "-"])
        .spawn()
        .map_err(|e| format!("Cannot start transcribe.exe beside the recorder. {e}"))?;
    let sent = serde_json::to_writer(
        child.stdin.take().ok_or("Missing worker input pipe")?,
        &input,
    );
    if let Err(e) = sent {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("Cannot send the worker request. {e}"));
    }
    // Control returns one bounded page. Do not read an unbounded reply into the GUI process.
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .ok_or("Missing worker output pipe")?
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() > 2 * 1024 * 1024 {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Cannot read a bounded worker response.".into());
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let reply: Value = serde_json::from_slice(&bytes).map_err(|_|format!("The worker returned an invalid response ({status}). Check that both executables came from the same package."))?;
    if status.success() && reply["ok"] == true {
        Ok(reply)
    } else {
        Err(reply["error"]["message"]
            .as_str()
            .unwrap_or("The worker could not complete the operation.")
            .to_string())
    }
}

/// An explicit endpoint workflow can still use the older worker. Its prompts and
/// output travel through bounded pipes to native controls.
pub(crate) struct EndpointSession {
    child: Child,
    input: SyncSender<String>,
    pub output: Receiver<Vec<u8>>,
}

impl EndpointSession {
    pub fn start(
        files: Option<&[PathBuf]>,
        config: Option<&TranscriptionConfig>,
    ) -> Result<Self, String> {
        let mut cmd = command()?;
        cmd.env(GUI_PID_ENV, std::process::id().to_string())
            .stderr(Stdio::piped());
        if let Some(files) = files {
            let snapshot = config
                .map(TranscriptionConfig::to_gui_snapshot)
                .transpose()?
                .unwrap_or_default();
            cmd.env(GUI_CONFIG_ENV, snapshot)
                .args(["--guided", "--"])
                .args(files);
        } else {
            cmd.args(["batch-queue", "manage"]);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Cannot start the endpoint worker. {e}"))?;
        let stdout = child.stdout.take().ok_or("Missing worker output")?;
        let stderr = child.stderr.take().ok_or("Missing worker errors")?;
        let mut stdin = child.stdin.take().ok_or("Missing worker input")?;
        let (send, output) = mpsc::sync_channel(32);
        fn drain(mut reader: impl Read, sender: SyncSender<Vec<u8>>) {
            let mut chunk = [0; 4096];
            while let Ok(n) = reader.read(&mut chunk) {
                if n == 0 || sender.send(chunk[..n].to_vec()).is_err() {
                    break;
                }
            }
        }
        let errors = send.clone();
        thread::spawn(move || drain(stdout, send));
        thread::spawn(move || drain(stderr, errors));
        let (input, answers) = mpsc::sync_channel::<String>(2);
        thread::spawn(move || {
            for answer in answers {
                if writeln!(stdin, "{answer}").is_err() || stdin.flush().is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            input,
            output,
        })
    }

    pub fn send(&self, answer: String) -> Result<(), String> {
        if answer.chars().any(char::is_control) {
            return Err("Enter one response at a time.".into());
        }
        self.input.try_send(answer).map_err(|e| match e {
            TrySendError::Full(_) => {
                "Wait for the next question before sending another reply.".into()
            }
            TrySendError::Disconnected(_) => "The endpoint worker has finished.".into(),
        })
    }

    pub fn done(&mut self) -> Result<Option<bool>, String> {
        self.child
            .try_wait()
            .map(|s| s.map(|s| s.success()))
            .map_err(|e| e.to_string())
    }
}

impl Drop for EndpointSession {
    fn drop(&mut self) {
        // Close a pending interactive session even if it is waiting on input.
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
    }
}
