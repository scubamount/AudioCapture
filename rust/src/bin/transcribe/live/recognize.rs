//! The network thread owns a single immutable WAV and never writes transcript state.
use crate::{jobs, transport};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

pub(super) struct Recognizer {
    pub endpoint: transport::Endpoint,
    pub model: String,
    pub key: Option<String>,
    pub language: Option<String>,
    pub timestamps: bool,
    pub provider: Option<transport::ProviderProfile>,
    pub cancel: Option<Arc<super::CancelEvent>>,
}
pub(super) struct Request {
    pub wav: Vec<u8>,
    pub deadline: Instant,
}
pub(super) struct Reply {
    pub result: Result<(String, Option<serde_json::Value>), String>,
}
pub(super) struct Service {
    pub send: SyncSender<Request>,
    pub recv: Receiver<Reply>,
}
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

// A single four-second response must fit the per-event journal budget, even when
// control characters expand in JSON and timed segments repeat the raw words.
const MAX_CHUNK_TEXT_BYTES: usize = 128 * 1024;

fn parse_chunk(reply: &transport::Response) -> Result<(String, Option<serde_json::Value>), String> {
    let result = jobs::parse_response(reply)?;
    if result.0.len() > MAX_CHUNK_TEXT_BYTES {
        return Err("Recognition text exceeds the live chunk limit".into());
    }
    Ok(result)
}

impl Service {
    pub fn new(config: Arc<Recognizer>) -> Result<Self, String> {
        let (send, requests) = sync_channel::<Request>(1);
        let (responses, recv) = sync_channel(1);
        std::thread::Builder::new()
            .name("live-recognition".into())
            .spawn(move || {
                while let Ok(req) = requests.recv() {
                    let result = transport::transcribe_wav(
                        &config.endpoint,
                        &config.model,
                        config.key.as_deref(),
                        config.language.as_deref(),
                        &req.wav,
                        config.timestamps,
                        config.provider,
                        req.deadline,
                        &|| {
                            config
                                .cancel
                                .as_deref()
                                .is_some_and(super::CancelEvent::signaled)
                        },
                    )
                    .map_err(|e| {
                        if e.may_have_been_processed {
                            format!("Remote outcome unknown: {e}")
                        } else {
                            e.to_string()
                        }
                    })
                    .and_then(|reply| parse_chunk(&reply));
                    if responses.send(Reply { result }).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| format!("Cannot start live recognition: {e}"))?;
        Ok(Self { send, recv })
    }
}

pub(super) fn wav(pcm: Vec<i16>) -> Vec<u8> {
    let bytes = (pcm.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + bytes as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(bytes + 36).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&48000u32.to_le_bytes());
    wav.extend_from_slice(&96000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    for sample in pcm {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_text_is_a_chunk_failure_before_journaling() {
        let reply = transport::Response {
            content_type: "text/plain".into(),
            body: vec![b'x'; MAX_CHUNK_TEXT_BYTES + 1],
        };
        assert!(parse_chunk(&reply).is_err());
    }
    #[test]
    fn memory_wav_preserves_every_sample() {
        let audio = wav(vec![-32768, 0, 32767]);
        assert_eq!(audio.len(), 50);
        assert_eq!(&audio[0..4], b"RIFF");
        assert_eq!(&audio[24..28], &48000u32.to_le_bytes());
        assert_eq!(&audio[40..44], &6u32.to_le_bytes());
        assert_eq!(&audio[44..], &[0, 128, 0, 0, 255, 127]);
    }
}
