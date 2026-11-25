//! A kill-on-close child isolates synchronous WinHTTP. Its supervisor owns
//! the request lease and enforces the absolute deadline across all I/O stages.
use crate::{attribution::adapter, jobs::Options, transport};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};

const MAX_HEADER: usize = 128 * 1024;
const MAX_BODY: usize = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Request {
    base_url: String,
    model: String,
    key_env: Option<String>,
    allow_http: bool,
    language: Option<String>,
    timestamps: bool,
    profile: Option<transport::ProviderProfile>,
    upload: PathBuf,
}

#[derive(Serialize, Deserialize)]
enum Reply {
    Received(String),
    Failed { message: String, unknown: bool },
}

fn error(msg: impl ToString, unknown: bool) -> transport::TransportError {
    transport::TransportError {
        message: msg.to_string(),
        may_have_been_processed: unknown,
    }
}

pub(super) fn child() -> Result<(), String> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take(MAX_HEADER as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_HEADER {
        return Err("Batch transfer options exceed the size limit".into());
    }
    let req: Request = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let send = || {
        let endpoint =
            transport::endpoint(&req.base_url, req.allow_http).map_err(|e| error(e, false))?;
        let key = req
            .key_env
            .as_ref()
            .map(std::env::var)
            .transpose()
            .map_err(|_| error("Named API key is unavailable", false))?;
        transport::transcribe_until(
            &endpoint,
            &req.model,
            key.as_deref(),
            req.language.as_deref(),
            &req.upload,
            req.timestamps,
            req.profile,
            Instant::now() + Duration::from_secs(120),
        )
    };
    let (header, body) = match send() {
        Ok(r) => (Reply::Received(r.content_type), r.body),
        Err(e) => (
            Reply::Failed {
                message: e.message,
                unknown: e.may_have_been_processed,
            },
            Vec::new(),
        ),
    };
    let mut out = io::BufWriter::new(io::stdout().lock());
    serde_json::to_writer(&mut out, &header).map_err(|e| e.to_string())?;
    out.write_all(b"\n")
        .and_then(|()| out.write_all(&body))
        .and_then(|()| out.flush())
        .map_err(|e| e.to_string())
}

fn read_reply(stdout: impl Read) -> Result<transport::Response, transport::TransportError> {
    let mut reader = BufReader::new(stdout);
    let mut header = Vec::new();
    (&mut reader)
        .take(MAX_HEADER as u64 + 1)
        .read_until(b'\n', &mut header)
        .map_err(|e| error(e, true))?;
    if header.len() > MAX_HEADER {
        return Err(error("Transfer response header exceeds its limit", true));
    }
    match serde_json::from_slice::<Reply>(&header)
        .map_err(|_| error("Batch transfer stopped without a valid response", true))?
    {
        Reply::Failed { message, unknown } => Err(error(message, unknown)),
        Reply::Received(content_type) => {
            let mut body = Vec::new();
            reader
                .take(MAX_BODY as u64 + 1)
                .read_to_end(&mut body)
                .map_err(|e| error(e, true))?;
            if body.len() > MAX_BODY {
                return Err(error("Transfer response exceeds the size limit", true));
            }
            Ok(transport::Response { content_type, body })
        }
    }
}

pub(crate) fn transfer(
    opts: &Options,
    upload: &Path,
    deadline: Instant,
) -> Result<transport::Response, transport::TransportError> {
    let req = Request {
        base_url: opts.base_url.clone(),
        model: opts.model.clone(),
        key_env: opts.key_env.clone(),
        allow_http: opts.allow_http,
        language: opts.language.clone(),
        timestamps: opts.timestamps,
        profile: opts.provider_profile,
        upload: upload.to_owned(),
    };
    let bytes = serde_json::to_vec(&req).map_err(|e| error(e, false))?;
    if bytes.len() > MAX_HEADER {
        return Err(error("Batch transfer options exceed the size limit", false));
    }
    let job = adapter::job().map_err(|e| error(e, false))?;
    let executable = std::env::current_exe().map_err(|e| error(e, false))?;
    let mut child = Command::new(executable)
        .args(["batch-queue", "--http-transfer"])
        .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| error(e, false))?;
    if let Err(e) = adapter::start_in_job(&child, &job) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error(e, false));
    }
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| error("Missing transfer input", false))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| error("Missing transfer output", false))?;
    let (sender, receiver) = mpsc::sync_channel(1);
    // The supervisor remains free even if the child stalls in an OS call or
    // stops reading its bounded setup before the first request.
    let io = std::thread::Builder::new()
        .name("batch-transfer-io".into())
        .spawn(move || {
            let result = stdin.write_all(&bytes).map_err(|e| error(e, false));
            drop(stdin);
            let _ = sender.send(result.and_then(|()| read_reply(stdout)));
        })
        .map_err(|e| error(e, false))?;
    let result = receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .unwrap_or_else(|_| {
            Err(error(
                "Batch request reached its absolute deadline or its transfer was interrupted.",
                true,
            ))
        });
    // The result stream is complete or the deadline expired. The child owns
    // no other work. Closing the job also covers abrupt supervisor exit.
    drop(job);
    let _ = child.wait();
    let _ = io.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_protocol_preserves_response_bytes_and_ambiguity() {
        let data = b"{\"Received\":\"text/plain\"}\nfirst line\nsecond line";
        assert_eq!(
            read_reply(&data[..]).unwrap().body,
            b"first line\nsecond line"
        );
        let failure = b"{\"Failed\":{\"message\":\"invalid file\",\"unknown\":false}}\n";
        assert!(
            !read_reply(&failure[..])
                .unwrap_err()
                .may_have_been_processed
        );
        assert!(
            read_reply(&b"truncated"[..])
                .unwrap_err()
                .may_have_been_processed
        );
    }
}
