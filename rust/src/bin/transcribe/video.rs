//! Audio-only import from a held local video file. No playback device participates.

mod mp4;
mod platform;
mod stream;

use std::fs::File;
use std::io::{self, Read, Seek};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::{Interface, PCWSTR, w};

const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const RATE: u32 = 16_000;
const MAX_SAMPLE_BYTES: u32 = 4 * 1024 * 1024;

/// Supply a MIME hint derived from the bytes. Never pass a file name or an arbitrary URI
/// to the media resolver. Text playlists renamed to a video extension fail here.
fn container_type(header: &[u8]) -> Option<PCWSTR> {
    if mp4::is_container(header) {
        Some(w!("video/mp4"))
    } else if header.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        if header.windows(4).any(|field| field == b"webm") {
            Some(w!("video/webm"))
        } else {
            Some(w!("video/x-matroska"))
        }
    } else if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"AVI ") {
        Some(w!("video/avi"))
    } else if header.starts_with(&[0, 0, 1, 0xba]) {
        Some(w!("video/mpeg"))
    } else if header.starts_with(&[
        0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62, 0xce,
        0x6c,
    ]) {
        Some(w!("video/x-ms-wmv"))
    } else {
        None
    }
}

struct MediaRuntime {
    owns_com: bool,
}

impl MediaRuntime {
    fn start() -> Result<Self, String> {
        platform::require_media_foundation()?;
        let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if init.is_err() && init != RPC_E_CHANGED_MODE {
            return Err(format!("Cannot initialize COM for video audio: {init}"));
        }
        let owns_com = init.is_ok();
        if let Err(error) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) } {
            if owns_com {
                unsafe { CoUninitialize() };
            }
            return Err(format!("Cannot start Windows Media Foundation: {error}"));
        }
        Ok(Self { owns_com })
    }
}

impl Drop for MediaRuntime {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            if self.owns_com {
                CoUninitialize();
            }
        }
    }
}

pub(super) struct PcmReader {
    reader: IMFSourceReader,
    bytes: io::Cursor<Vec<u8>>,
    done: bool,
    produced_audio: bool,
    // Drop the COM reader before shutting down its runtime on this thread.
    _runtime: MediaRuntime,
}

impl PcmReader {
    pub(super) fn open(input: &File) -> Result<Self, String> {
        let mut file = input
            .try_clone()
            .map_err(|e| format!("Cannot duplicate video file: {e}"))?;
        let mut header = [0; 128];
        let count = file
            .rewind()
            .and_then(|()| file.read(&mut header))
            .map_err(|e| format!("Cannot inspect video container: {e}"))?;
        let mime = container_type(&header[..count])
            .ok_or("Unsupported or invalid local video container")?;
        let hidden = if mp4::is_container(&header[..count]) {
            mp4::additional_audio_tracks(&mut file)
                .map_err(|e| format!("Cannot inspect MP4 audio tracks: {e}"))?
        } else {
            Vec::new()
        };
        let source =
            stream::from_file(file, hidden).map_err(|e| format!("Cannot read video file: {e}"))?;
        let runtime = MediaRuntime::start()?;
        let reader = unsafe {
            (|| -> windows_core::Result<IMFSourceReader> {
                let bytes = MFCreateMFByteStreamOnStream(&source)?;
                bytes
                    .cast::<IMFAttributes>()?
                    .SetString(&MF_BYTESTREAM_CONTENT_TYPE, mime)?;
                MFCreateSourceReaderFromByteStream(&bytes, None)
            })()
        }
        .map_err(|e| format!("Windows cannot open this video container: {e}"))?;
        unsafe {
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(|e| format!("Cannot disable video streams: {e}"))?;
            reader
                .GetNativeMediaType(AUDIO, 0)
                .map_err(|e| format!("Video has no readable embedded audio track: {e}"))?;
            (|| -> windows_core::Result<()> {
                reader.SetStreamSelection(AUDIO, true)?;
                let target = MFCreateMediaType()?;
                target.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                target.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
                target.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 1)?;
                target.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
                target.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                target.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 2)?;
                target.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, RATE * 2)?;
                reader.SetCurrentMediaType(AUDIO, None, &target)?;
                check_format(&reader)
            })()
        }
        .map_err(|e| format!("Windows cannot decode the first audio track to mono PCM: {e}"))?;
        Ok(Self {
            reader,
            bytes: io::Cursor::new(Vec::new()),
            done: false,
            produced_audio: false,
            _runtime: runtime,
        })
    }

    fn next_sample(&mut self) -> io::Result<()> {
        let mut flags = 0;
        let mut sample = None;
        unsafe {
            self.reader
                .ReadSample(AUDIO, 0, None, Some(&mut flags), None, Some(&mut sample))
        }
        .map_err(io::Error::other)?;
        if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
            return Err(io::Error::other("Windows reported an audio decoder error"));
        }
        if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
            check_format(&self.reader).map_err(io::Error::other)?;
        }
        self.done = flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0;
        if let Some(sample) = sample {
            let size = unsafe { sample.GetTotalLength() }.map_err(io::Error::other)?;
            if size > MAX_SAMPLE_BYTES || size % 2 != 0 {
                return Err(io::Error::other("Invalid PCM audio sample length"));
            }
            if size > 0 {
                let buffer =
                    unsafe { sample.ConvertToContiguousBuffer() }.map_err(io::Error::other)?;
                let mut data = std::ptr::null_mut();
                let mut length = 0;
                unsafe { buffer.Lock(&mut data, None, Some(&mut length)) }
                    .map_err(io::Error::other)?;
                let valid = !data.is_null() && length == size;
                if valid {
                    let bytes = self.bytes.get_mut();
                    bytes.clear();
                    bytes.extend_from_slice(unsafe {
                        std::slice::from_raw_parts(data, length as usize)
                    });
                    self.bytes.set_position(0);
                }
                unsafe { buffer.Unlock() }.map_err(io::Error::other)?;
                if !valid {
                    return Err(io::Error::other("Invalid Windows audio sample buffer"));
                }
                self.produced_audio = true;
            }
        }
        if self.done && !self.produced_audio {
            return Err(io::Error::other(
                "The embedded audio track contains no decoded samples",
            ));
        }
        Ok(())
    }
}

fn check_format(reader: &IMFSourceReader) -> windows_core::Result<()> {
    unsafe {
        let current = reader.GetCurrentMediaType(AUDIO)?;
        if current.GetGUID(&MF_MT_SUBTYPE)? == MFAudioFormat_PCM
            && current.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)? == 1
            && current.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)? == RATE
            && current.GetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE)? == 16
        {
            Ok(())
        } else {
            Err(MF_E_INVALIDMEDIATYPE.into())
        }
    }
}

impl Read for PcmReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        loop {
            let count = self.bytes.read(output)?;
            if count > 0 || self.done || output.is_empty() {
                return Ok(count);
            }
            self.next_sample()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_playlists_and_accepts_binary_video_headers() {
        assert!(container_type(b"#EXTM3U\nhttps://example.invalid/a.ts").is_none());
        assert!(container_type(b"<asx version='3'><entry>").is_none());
        assert!(container_type(b"RIFF1234WAVE").is_none());
        assert!(container_type(b"\0\0\0\x1cftypisom").is_some());
        assert!(container_type(b"RIFF1234AVI ").is_some());
    }
}
