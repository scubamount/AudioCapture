//! Bounded, streaming access to an OpenAI-compatible transcription endpoint.
//!
//! WinHTTP validates TLS certificates. Redirects, ambient proxy settings, and
//! automatic retries are disabled so audio and credentials stay with the
//! endpoint chosen by the user.

use std::ffi::c_void;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub use audio_capture::transcription_endpoint::{Endpoint, canonical_key, endpoint};
use windows_sys::Win32::Networking::WinHttp::*;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const BLOCK_SIZE: usize = 64 * 1024;
static NEXT_BOUNDARY: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum ProviderProfile {
    #[serde(rename = "openai-diarized-v1")]
    OpenAiDiarizedV1,
}

impl ProviderProfile {
    pub const OPENAI_DIARIZED_V1: &str = "openai-diarized-v1";
}

impl FromStr for ProviderProfile {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            Self::OPENAI_DIARIZED_V1 => Ok(Self::OpenAiDiarizedV1),
            _ => Err(format!(
                "Unknown provider profile. Choose {} for a compatible endpoint and model.",
                Self::OPENAI_DIARIZED_V1
            )),
        }
    }
}

#[derive(Debug)]
pub struct Response {
    pub content_type: String,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub struct TransportError {
    pub message: String,
    pub may_have_been_processed: bool,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TransportError {}

fn error(message: impl Into<String>, may_have_been_processed: bool) -> TransportError {
    TransportError {
        message: message.into(),
        may_have_been_processed,
    }
}

fn windows_error(operation: &str, may_have_been_processed: bool) -> TransportError {
    error(
        format!("{operation}: {}", std::io::Error::last_os_error()),
        may_have_been_processed,
    )
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

struct Handle(*mut c_void);

impl Handle {
    fn new(handle: *mut c_void, operation: &str) -> Result<Self, TransportError> {
        if handle.is_null() {
            Err(windows_error(operation, false))
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn valid_field(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_control)
}

fn file_type(path: &Path) -> Result<&'static str, TransportError> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    match ext.to_ascii_lowercase().as_str() {
        "opus" => Ok("opus"),
        "ogg" => Ok("ogg"),
        "wav" => Ok("wav"),
        "mp3" => Ok("mp3"),
        "flac" => Ok("flac"),
        "m4a" => Ok("m4a"),
        "mp4" => Ok("mp4"),
        "mpeg" | "mpga" => Ok("mpeg"),
        "webm" => Ok("webm"),
        "mkv" | "mka" => Ok("mka"),
        _ => Err(error("Unsupported audio file extension", false)),
    }
}

fn add_field(buffer: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    buffer.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
        .as_bytes(),
    );
}

fn write_all(
    request: &Handle,
    mut data: &[u8],
    deadline: Option<Instant>,
) -> Result<(), TransportError> {
    while !data.is_empty() {
        if deadline.is_some() {
            request_timeouts(request, deadline, true)?;
        }
        let mut written = 0;
        let length = data.len().min(u32::MAX as usize) as u32;
        let ok = unsafe { WinHttpWriteData(request.0, data.as_ptr().cast(), length, &mut written) };
        if ok == 0 || written == 0 {
            return Err(windows_error("Cannot send audio", true));
        }
        data = &data[written as usize..];
    }
    Ok(())
}

fn query_status(request: &Handle) -> Result<u32, TransportError> {
    let mut status = 0u32;
    let mut length = std::mem::size_of::<u32>() as u32;
    let ok = unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut length,
            null_mut(),
        )
    };
    if ok == 0 {
        return Err(windows_error("Cannot read HTTP status", true));
    }
    Ok(status)
}

fn query_content_type(request: &Handle) -> Result<String, TransportError> {
    let mut buffer = [0u16; 2048];
    let mut bytes = std::mem::size_of_val(&buffer) as u32;
    let ok = unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_CONTENT_TYPE,
            null(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
            null_mut(),
        )
    };
    if ok == 0 {
        return Err(windows_error("Cannot read response content type", true));
    }
    let count = (bytes as usize / 2).min(buffer.len());
    let content_type = String::from_utf16_lossy(&buffer[..count])
        .trim_end_matches('\0')
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if content_type != "text/plain"
        && content_type != "application/json"
        && !content_type.ends_with("+json")
    {
        return Err(error("Endpoint returned an unsupported content type", true));
    }
    Ok(content_type)
}

/// Post a single selected audio file. The caller controls batching and retries.
pub fn transcribe(
    endpoint: &Endpoint,
    model: &str,
    key: Option<&str>,
    language: Option<&str>,
    file: &Path,
    timestamps: bool,
    profile: Option<ProviderProfile>,
) -> Result<Response, TransportError> {
    transcribe_file(
        endpoint, model, key, language, file, timestamps, profile, None,
    )
}

pub fn transcribe_until(
    endpoint: &Endpoint,
    model: &str,
    key: Option<&str>,
    language: Option<&str>,
    file: &Path,
    timestamps: bool,
    profile: Option<ProviderProfile>,
    deadline: Instant,
) -> Result<Response, TransportError> {
    transcribe_file(
        endpoint,
        model,
        key,
        language,
        file,
        timestamps,
        profile,
        Some(deadline),
    )
}

#[allow(clippy::too_many_arguments)]
fn transcribe_file(
    endpoint: &Endpoint,
    model: &str,
    key: Option<&str>,
    language: Option<&str>,
    file: &Path,
    timestamps: bool,
    profile: Option<ProviderProfile>,
    deadline: Option<Instant>,
) -> Result<Response, TransportError> {
    let extension = file_type(file)?;
    let mut input =
        File::open(file).map_err(|e| error(format!("Cannot open audio: {e}"), false))?;
    let metadata = input
        .metadata()
        .map_err(|e| error(format!("Cannot inspect audio: {e}"), false))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(error("Audio must be a nonempty regular file", false));
    }
    post_audio(
        endpoint,
        model,
        key,
        language,
        extension,
        &mut input,
        metadata.len(),
        timestamps,
        profile,
        deadline,
        None,
    )
}

/// Live chunks remain in memory. The filename never includes the original source identity.
pub(super) fn transcribe_wav(
    endpoint: &Endpoint,
    model: &str,
    key: Option<&str>,
    language: Option<&str>,
    mut wav: &[u8],
    timestamps: bool,
    profile: Option<ProviderProfile>,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Result<Response, TransportError> {
    let length = wav.len() as u64;
    post_audio(
        endpoint,
        model,
        key,
        language,
        "wav",
        &mut wav,
        length,
        timestamps,
        profile,
        Some(deadline),
        Some(cancelled),
    )
}

fn request_timeouts(
    handle: &Handle,
    deadline: Option<Instant>,
    sent: bool,
) -> Result<(), TransportError> {
    let cap = match deadline {
        Some(at) => at
            .checked_duration_since(Instant::now())
            .map(|d| d.as_millis().clamp(1, i32::MAX as u128) as i32)
            .ok_or_else(|| error("Transcription request reached its deadline", sent))?,
        None => i32::MAX,
    };
    if unsafe {
        WinHttpSetTimeouts(
            handle.0,
            15_000.min(cap),
            30_000.min(cap),
            120_000.min(cap),
            120_000.min(cap),
        )
    } == 0
    {
        return Err(windows_error("Cannot configure HTTP timeouts", sent));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn post_audio(
    endpoint: &Endpoint,
    model: &str,
    key: Option<&str>,
    language: Option<&str>,
    extension: &str,
    input: &mut dyn Read,
    length: u64,
    timestamps: bool,
    profile: Option<ProviderProfile>,
    deadline: Option<Instant>,
    cancelled: Option<&dyn Fn() -> bool>,
) -> Result<Response, TransportError> {
    if timestamps && profile.is_some() {
        return Err(error(
            "Choose either timestamps or a provider profile",
            false,
        ));
    }
    if !valid_field(model) || language.is_some_and(|value| !valid_field(value)) {
        return Err(error("Invalid model or language", false));
    }
    if key
        .is_some_and(|value| value.is_empty() || !value.bytes().all(|c| (0x21..=0x7e).contains(&c)))
    {
        return Err(error("Invalid API key", false));
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error("System clock is invalid", false))?
        .as_nanos();
    let boundary = format!(
        "audio-capture-{nonce:x}-{:x}",
        NEXT_BOUNDARY.fetch_add(1, Ordering::Relaxed)
    );
    let mut prefix = Vec::new();
    add_field(&mut prefix, &boundary, "model", model);
    if let Some(language) = language {
        add_field(&mut prefix, &boundary, "language", language);
    }
    match profile {
        Some(ProviderProfile::OpenAiDiarizedV1) => {
            add_field(&mut prefix, &boundary, "response_format", "diarized_json");
            add_field(&mut prefix, &boundary, "chunking_strategy", "auto");
        }
        None if timestamps => {
            add_field(&mut prefix, &boundary, "response_format", "verbose_json");
        }
        None => {}
    }
    prefix.extend_from_slice(format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"recording.{extension}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    ).as_bytes());
    let suffix = format!("\r\n--{boundary}--\r\n");
    let total = (prefix.len() as u64)
        .checked_add(length)
        .and_then(|n| n.checked_add(suffix.len() as u64))
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| error("Audio exceeds WinHTTP request size", false))?;

    let agent = wide("AudioCapture transcription");
    let session = Handle::new(
        unsafe {
            WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                null(),
                null(),
                0,
            )
        },
        "Cannot initialize HTTP",
    )?;
    request_timeouts(&session, deadline, false)?;
    let host = wide(&endpoint.host);
    let connection = Handle::new(
        unsafe { WinHttpConnect(session.0, host.as_ptr(), endpoint.port, 0) },
        "Cannot connect to endpoint",
    )?;
    let method = wide("POST");
    let path = wide(&endpoint.path);
    let request = Handle::new(
        unsafe {
            WinHttpOpenRequest(
                connection.0,
                method.as_ptr(),
                path.as_ptr(),
                null(),
                null(),
                null(),
                if endpoint.secure {
                    WINHTTP_FLAG_SECURE
                } else {
                    0
                },
            )
        },
        "Cannot create transcription request",
    )?;
    let policy = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
    if unsafe {
        WinHttpSetOption(
            request.0,
            WINHTTP_OPTION_REDIRECT_POLICY,
            (&policy as *const u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    } == 0
    {
        return Err(windows_error("Cannot disable redirects", false));
    }
    let disabled = WINHTTP_DISABLE_AUTHENTICATION | WINHTTP_DISABLE_REDIRECTS;
    if unsafe {
        WinHttpSetOption(
            request.0,
            WINHTTP_OPTION_DISABLE_FEATURE,
            (&disabled as *const u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    } == 0
    {
        return Err(windows_error(
            "Cannot disable automatic HTTP authentication",
            false,
        ));
    }
    let mut headers = format!("Content-Type: multipart/form-data; boundary={boundary}\r\n");
    if let Some(key) = key {
        headers.push_str(&format!("Authorization: Bearer {key}\r\n"));
    }
    let headers = wide(&headers);
    if deadline.is_some() {
        request_timeouts(&request, deadline, false)?;
    }
    // Finalization can cancel work after the journal commit and before this thread
    // starts sending. A cancelled, unsent request has no unknown remote outcome.
    if cancelled.is_some_and(|check| check()) {
        return Err(error(
            "Transcription request cancelled before submission",
            false,
        ));
    }
    if unsafe { WinHttpSendRequest(request.0, headers.as_ptr(), u32::MAX, null(), 0, total, 0) }
        == 0
    {
        return Err(windows_error("Cannot begin transcription request", true));
    }
    write_all(&request, &prefix, deadline)?;
    let mut buffer = [0u8; BLOCK_SIZE];
    let mut remaining = length;
    while remaining > 0 {
        let count = input
            .read(&mut buffer[..remaining.min(BLOCK_SIZE as u64) as usize])
            .map_err(|e| error(format!("Cannot read audio: {e}"), true))?;
        if count == 0 {
            return Err(error("Audio changed during upload", true));
        }
        write_all(&request, &buffer[..count], deadline)?;
        remaining -= count as u64;
    }
    write_all(&request, suffix.as_bytes(), deadline)?;
    if deadline.is_some() {
        request_timeouts(&request, deadline, true)?;
    }
    if unsafe { WinHttpReceiveResponse(request.0, null_mut()) } == 0 {
        return Err(windows_error("Cannot receive transcription response", true));
    }
    let status = query_status(&request)?;
    if !(200..300).contains(&status) {
        return Err(error(
            format!("Transcription endpoint returned HTTP {status}"),
            status == 408 || status == 429 || status >= 500,
        ));
    }
    let content_type = query_content_type(&request)?;
    let mut body = Vec::new();
    loop {
        if deadline.is_some() {
            request_timeouts(&request, deadline, true)?;
        }
        let mut read = 0;
        if unsafe {
            WinHttpReadData(
                request.0,
                buffer.as_mut_ptr().cast(),
                BLOCK_SIZE as u32,
                &mut read,
            )
        } == 0
        {
            return Err(windows_error("Cannot read transcription response", true));
        }
        if read == 0 {
            break;
        }
        if body.len() + read as usize > MAX_RESPONSE_BYTES {
            return Err(error("Transcription response exceeds the size limit", true));
        }
        body.extend_from_slice(&buffer[..read as usize]);
    }
    if deadline.is_some_and(|at| Instant::now() >= at) {
        return Err(error("Transcription request reached its deadline", true));
    }
    Ok(Response { content_type, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn cancellation_after_journaling_sends_no_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let target = endpoint(
            &format!("http://{}/v1", listener.local_addr().unwrap()),
            false,
        )
        .unwrap();
        let result = transcribe_wav(
            &target,
            "fixture",
            None,
            None,
            b"bounded audio",
            false,
            None,
            Instant::now() + std::time::Duration::from_secs(2),
            &|| true,
        )
        .unwrap_err();
        assert!(!result.may_have_been_processed);
        assert!(result.message.contains("cancelled before submission"));
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn endpoint_checks_prefix_and_transport() {
        let target = endpoint("http://127.0.0.1:1234/gateway/v1/", false).unwrap();
        assert_eq!(target.path, "/gateway/v1/audio/transcriptions");
        assert_eq!(target.port, 1234);
        assert!(endpoint("http://example.com/v1", false).is_err());
        assert!(endpoint("http://example.com/v1", true).is_ok());
        assert!(endpoint("https://user:secret@example.com/v1", false).is_err());
        assert!(endpoint("https://example.com/v1?key=secret", false).is_err());
        assert!(endpoint("http://[::1]:42/v1", false).is_ok());
        assert!(endpoint("https://example.com/x/../v1", false).is_err());
        assert_eq!(
            canonical_key(&endpoint("https://EXAMPLE.com/v1/", false).unwrap()),
            canonical_key(&endpoint("https://example.com:443/v1", false).unwrap())
        );
        assert_eq!(
            canonical_key(&endpoint("http://[::1]:42/v1", false).unwrap()),
            "http://[::1]:42/v1/audio/transcriptions"
        );
    }

    #[test]
    fn posts_a_prefixed_multipart_request_to_a_local_endpoint() {
        for (timestamps, profile) in [
            (false, None),
            (true, None),
            (false, Some(ProviderProfile::OpenAiDiarizedV1)),
        ] {
            assert_prefixed_multipart(timestamps, profile);
        }
    }

    fn assert_prefixed_multipart(timestamps: bool, profile: Option<ProviderProfile>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            let header_end = loop {
                let n = stream.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
            assert!(headers.starts_with("POST /gateway/v1/audio/transcriptions HTTP/1.1\r\n"));
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer test-token")
            );
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            while bytes.len() - header_end < length {
                let n = stream.read(&mut buffer).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
            }
            let body = String::from_utf8(bytes[header_end..].to_vec()).unwrap();
            assert!(body.contains("name=\"model\"\r\n\r\nlocal-model\r\n"));
            assert_eq!(
                body.contains("name=\"response_format\"\r\n\r\nverbose_json\r\n"),
                timestamps
            );
            assert_eq!(
                body.contains("name=\"response_format\"\r\n\r\ndiarized_json\r\n"),
                profile.is_some()
            );
            assert_eq!(
                body.contains("name=\"chunking_strategy\"\r\n\r\nauto\r\n"),
                profile.is_some()
            );
            assert!(body.contains("filename=\"recording.opus\""));
            assert!(body.contains("synthetic-audio"));
            let response = b"{\"text\":\"hello\"}";
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len()).unwrap();
            stream.write_all(response).unwrap();
        });
        let file = std::env::temp_dir().join(format!(
            "audio-capture-http-test-{}.opus",
            std::process::id()
        ));
        std::fs::write(&file, b"synthetic-audio").unwrap();
        let target = endpoint(&format!("http://{address}/gateway/v1"), false).unwrap();
        let result = transcribe(
            &target,
            "local-model",
            Some("test-token"),
            Some("en"),
            &file,
            timestamps,
            profile,
        );
        std::fs::remove_file(&file).unwrap();
        let response = result.unwrap();
        assert_eq!(response.content_type, "application/json");
        assert_eq!(response.body, b"{\"text\":\"hello\"}");
        worker.join().unwrap();
    }

    #[test]
    fn rejects_redirects_and_non_transcript_responses() {
        for (status, headers, expected) in [
            (
                "307 Temporary Redirect",
                "Location: http://127.0.0.1:1/other\r\n",
                "HTTP 307",
            ),
            (
                "200 OK",
                "Content-Type: text/html\r\n",
                "unsupported content type",
            ),
            ("503 Service Unavailable", "", "HTTP 503"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let reply = format!(
                "HTTP/1.1 {status}\r\n{headers}Content-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let worker = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                let end = loop {
                    let n = stream.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                while bytes.len() - end < length {
                    let n = stream.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                }
                stream.write_all(reply.as_bytes()).unwrap();
            });
            let file = std::env::temp_dir().join(format!(
                "audio-capture-http-reject-{}-{}.opus",
                std::process::id(),
                address.port()
            ));
            std::fs::write(&file, b"synthetic-audio").unwrap();
            let target = endpoint(&format!("http://{address}/v1"), false).unwrap();
            let failure = transcribe(&target, "local", None, None, &file, false, None).unwrap_err();
            std::fs::remove_file(&file).unwrap();
            assert!(failure.message.contains(expected), "{}", failure.message);
            if status.starts_with("503") {
                assert!(failure.may_have_been_processed);
            }
            worker.join().unwrap();
        }
    }
}
