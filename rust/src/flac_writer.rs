//! Streaming FLAC output using a statically linked libFLAC encoder.

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::ptr;

use libflac_sys as flac;

struct Output {
    file: File,
    error: Option<String>,
}

unsafe extern "C" fn write_callback(
    _: *const flac::FLAC__StreamEncoder,
    buffer: *const u8,
    bytes: usize,
    _: u32,
    _: u32,
    context: *mut c_void,
) -> flac::FLAC__StreamEncoderWriteStatus {
    let output = unsafe { &mut *(context as *mut Output) };
    let data = unsafe { std::slice::from_raw_parts(buffer, bytes) };
    match output.file.write_all(data) {
        Ok(()) => flac::FLAC__STREAM_ENCODER_WRITE_STATUS_OK,
        Err(error) => {
            output.error = Some(error.to_string());
            flac::FLAC__STREAM_ENCODER_WRITE_STATUS_FATAL_ERROR
        }
    }
}

unsafe extern "C" fn seek_callback(
    _: *const flac::FLAC__StreamEncoder,
    position: u64,
    context: *mut c_void,
) -> flac::FLAC__StreamEncoderSeekStatus {
    let output = unsafe { &mut *(context as *mut Output) };
    match output.file.seek(SeekFrom::Start(position)) {
        Ok(_) => flac::FLAC__STREAM_ENCODER_SEEK_STATUS_OK,
        Err(error) => {
            output.error = Some(error.to_string());
            flac::FLAC__STREAM_ENCODER_SEEK_STATUS_ERROR
        }
    }
}

unsafe extern "C" fn tell_callback(
    _: *const flac::FLAC__StreamEncoder,
    position: *mut u64,
    context: *mut c_void,
) -> flac::FLAC__StreamEncoderTellStatus {
    let output = unsafe { &mut *(context as *mut Output) };
    match output.file.stream_position() {
        Ok(offset) => {
            unsafe { *position = offset };
            flac::FLAC__STREAM_ENCODER_TELL_STATUS_OK
        }
        Err(error) => {
            output.error = Some(error.to_string());
            flac::FLAC__STREAM_ENCODER_TELL_STATUS_ERROR
        }
    }
}

/// Accepts interleaved 48 kHz stereo floating-point samples.
pub struct FlacWriter {
    encoder: *mut flac::FLAC__StreamEncoder,
    output: Box<Output>,
    channels: u16,
    initialized: bool,
    converted: Vec<i32>,
}

impl FlacWriter {
    pub fn open(path: &Path, channels: u16, level: u8) -> Result<Self, String> {
        if !(channels == 1 || channels == 2) || level > 8 {
            return Err("FLAC requires one or two channels and compression level 0 to 8".into());
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
        let encoder = unsafe { flac::FLAC__stream_encoder_new() };
        if encoder.is_null() {
            return Err("Cannot allocate FLAC encoder".into());
        }
        let mut writer = Self {
            encoder,
            output: Box::new(Output { file, error: None }),
            channels,
            initialized: false,
            converted: Vec::new(),
        };
        unsafe {
            if flac::FLAC__stream_encoder_set_channels(encoder, channels as u32) == 0
                || flac::FLAC__stream_encoder_set_bits_per_sample(encoder, 16) == 0
                || flac::FLAC__stream_encoder_set_sample_rate(encoder, 48_000) == 0
                || flac::FLAC__stream_encoder_set_compression_level(encoder, level as u32) == 0
            {
                return Err("Cannot configure FLAC encoder".into());
            }
            let result = flac::FLAC__stream_encoder_init_stream(
                encoder,
                Some(write_callback),
                Some(seek_callback),
                Some(tell_callback),
                None,
                (&mut *writer.output as *mut Output).cast(),
            );
            if result != flac::FLAC__STREAM_ENCODER_INIT_STATUS_OK {
                return Err(format!("Cannot initialize FLAC encoder: status {result}"));
            }
        }
        writer.initialized = true;
        Ok(writer)
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        if samples.len() % 2 != 0 {
            return Err("FLAC input contains an incomplete stereo frame".into());
        }
        self.converted.clear();
        self.converted
            .reserve(samples.len() / 2 * usize::from(self.channels));
        for frame in samples.chunks_exact(2) {
            if self.channels == 1 {
                self.converted.push(to_pcm((frame[0] + frame[1]) * 0.5));
            } else {
                self.converted.push(to_pcm(frame[0]));
                self.converted.push(to_pcm(frame[1]));
            }
        }
        let frame_count = u32::try_from(samples.len() / 2)
            .map_err(|_| "FLAC input block is too large".to_string())?;
        if unsafe {
            flac::FLAC__stream_encoder_process_interleaved(
                self.encoder,
                self.converted.as_ptr(),
                frame_count,
            )
        } == 0
        {
            return Err(self.error("FLAC encoding failed"));
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), String> {
        let success = unsafe { flac::FLAC__stream_encoder_finish(self.encoder) };
        self.initialized = false;
        if success == 0 {
            return Err(self.error("Cannot finalize FLAC output"));
        }
        self.output
            .file
            .flush()
            .map_err(|e| format!("Cannot flush FLAC output: {e}"))
    }

    fn error(&self, prefix: &str) -> String {
        match &self.output.error {
            Some(error) => format!("{prefix}: {error}"),
            None => prefix.into(),
        }
    }
}

fn to_pcm(sample: f32) -> i32 {
    if !sample.is_finite() {
        return 0;
    }
    (sample.clamp(-1.0, 1.0) * 32767.0).round() as i32
}

impl Drop for FlacWriter {
    fn drop(&mut self) {
        if !self.encoder.is_null() {
            if self.initialized {
                unsafe { flac::FLAC__stream_encoder_finish(self.encoder) };
            }
            unsafe { flac::FLAC__stream_encoder_delete(self.encoder) };
            self.encoder = ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn encodes_mono_and_stereo_with_exact_duration() {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("flac-test-{}-{id}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();

        for channels in [1, 2] {
            let path = dir.join(format!("音声-{channels}.flac"));
            let mut writer = FlacWriter::open(&path, channels, 8).unwrap();
            assert!(FlacWriter::open(&path, channels, 8).is_err());
            let input: Vec<f32> = (0..48_000)
                .flat_map(|i| {
                    let sample =
                        ((i as f32 * 440.0 * std::f32::consts::TAU) / 48_000.0).sin() * 0.5;
                    [sample, sample]
                })
                .collect();
            writer.write(&input).unwrap();
            assert!(writer.write(&[0.0]).is_err());
            writer.finish().unwrap();

            let ffprobe = Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "stream=sample_rate,channels,duration",
                    "-of",
                    "default=noprint_wrappers=1",
                ])
                .arg(&path)
                .output();
            if let Ok(output) = ffprobe {
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let details = String::from_utf8(output.stdout).unwrap();
                assert!(details.contains("sample_rate=48000"));
                assert!(details.contains(&format!("channels={channels}")));
                assert!(details.contains("duration=1.000000"));
            }
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(&dir).unwrap();
    }
}
