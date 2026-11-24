//! A streaming MP3 writer backed by the Windows Media Foundation encoder.
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::PCWSTR;

const RATE: u32 = 48_000;
const TICKS_PER_SECOND: u64 = 10_000_000;

pub struct Mp3Writer {
    sink: Option<IMFSinkWriter>,
    stream: u32,
    channels: u16,
    frames: u64,
    com_initialized: bool,
    mf_started: bool,
}

impl Mp3Writer {
    pub fn open(path: &Path, channels: u16, bitrate: u32) -> Result<Self, String> {
        if !matches!(channels, 1 | 2) || !matches!(bitrate, 128 | 192 | 256 | 320) {
            return Err("Unsupported MP3 channel count or bitrate".into());
        }
        let mut writer = Self {
            sink: None,
            stream: 0,
            channels,
            frames: 0,
            com_initialized: false,
            mf_started: false,
        };
        let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if initialized.is_ok() {
            writer.com_initialized = true;
        } else if initialized.0 != 0x80010106u32 as i32 {
            return Err(format!("Cannot initialize COM: {initialized:?}"));
        }
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL)
                .map_err(|e| format!("Cannot start Media Foundation: {e}"))?;
        }
        writer.mf_started = true;
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.push(0);
        let result = unsafe {
            (|| -> windows::core::Result<()> {
                let stream = MFCreateFile(
                    MF_ACCESSMODE_WRITE,
                    MF_OPENMODE_FAIL_IF_EXIST,
                    MF_FILEFLAGS_NONE,
                    PCWSTR(wide.as_ptr()),
                )?;
                let sink = MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), &stream, None)?;
                let output = MFCreateMediaType()?;
                output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                output.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_MP3)?;
                output.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels as u32)?;
                output.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
                output.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bitrate * 1000 / 8)?;
                let stream = sink.AddStream(&output)?;
                let input = MFCreateMediaType()?;
                input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
                input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels as u32)?;
                input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
                input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, channels as u32 * 2)?;
                input.SetUINT32(
                    &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
                    RATE * channels as u32 * 2,
                )?;
                sink.SetInputMediaType(stream, &input, None)?;
                sink.BeginWriting()?;
                writer.stream = stream;
                writer.sink = Some(sink);
                Ok(())
            })()
        };
        result.map_err(|e| format!("Cannot initialize MP3 encoder: {e}"))?;
        Ok(writer)
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        if samples.len() % 2 != 0 {
            return Err("Input must contain complete stereo frames".into());
        }
        for chunk in samples.chunks(960) {
            self.write_chunk(chunk)?;
        }
        Ok(())
    }

    fn write_chunk(&mut self, samples: &[f32]) -> Result<(), String> {
        if samples.is_empty() {
            return Ok(());
        }
        let frames = samples.len() / 2;
        let mut pcm = Vec::with_capacity(frames * self.channels as usize * 2);
        for stereo in samples.chunks_exact(2) {
            let values: &[f32] = if self.channels == 1 { &[] } else { stereo };
            if self.channels == 1 {
                let mono = (stereo[0] + stereo[1]) * 0.5;
                pcm.extend_from_slice(&to_i16(mono).to_le_bytes());
            } else {
                for &value in values {
                    pcm.extend_from_slice(&to_i16(value).to_le_bytes());
                }
            }
        }
        let bytes = u32::try_from(pcm.len()).map_err(|_| "MP3 input chunk is too large")?;
        unsafe {
            let buffer = MFCreateMemoryBuffer(bytes).map_err(|e| e.to_string())?;
            let mut target = std::ptr::null_mut();
            buffer
                .Lock(&mut target, None, None)
                .map_err(|e| e.to_string())?;
            std::ptr::copy_nonoverlapping(pcm.as_ptr(), target, pcm.len());
            buffer.Unlock().map_err(|e| e.to_string())?;
            buffer.SetCurrentLength(bytes).map_err(|e| e.to_string())?;
            let sample = MFCreateSample().map_err(|e| e.to_string())?;
            sample.AddBuffer(&buffer).map_err(|e| e.to_string())?;
            let end = self
                .frames
                .checked_add(frames as u64)
                .ok_or("MP3 duration overflow")?;
            let start_time = self.frames * TICKS_PER_SECOND / RATE as u64;
            let end_time = end * TICKS_PER_SECOND / RATE as u64;
            sample
                .SetSampleTime(start_time as i64)
                .map_err(|e| e.to_string())?;
            sample
                .SetSampleDuration((end_time - start_time) as i64)
                .map_err(|e| e.to_string())?;
            self.sink
                .as_ref()
                .ok_or("MP3 writer is closed")?
                .WriteSample(self.stream, &sample)
                .map_err(|e| format!("MP3 write failed: {e}"))?;
            self.frames = end;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), String> {
        // Media Foundation retains the last MP3 frames internally. Give the
        // encoder extra silence, and enough frames for a valid short file.
        let padded_end = (self.frames + 9 * 1_152).max(9 * 1_152);
        let silence_frames = padded_end - self.frames;
        self.write(&vec![0.0; silence_frames as usize * 2])?;
        let sink = self.sink.take().ok_or("MP3 writer is closed")?;
        unsafe {
            sink.Finalize()
                .map_err(|e| format!("MP3 finalization failed: {e}"))
        }
    }
}

impl Drop for Mp3Writer {
    fn drop(&mut self) {
        self.sink.take();
        unsafe {
            if self.mf_started {
                let _ = MFShutdown();
            }
            if self.com_initialized {
                CoUninitialize();
            }
        }
    }
}

fn to_i16(value: f32) -> i16 {
    (value.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn final_signal_survives_and_existing_file_is_preserved() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            eprintln!("Skipping MP3 decode test because FFmpeg is unavailable");
            return;
        }
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("audio-capture-mp3-{id}.mp3"));
        let mut writer = Mp3Writer::open(&path, 1, 128).unwrap();
        let mut signal = vec![0.0; 48_000 * 2];
        for sample in &mut signal[(48_000 - 480) * 2..] {
            *sample = 0.8;
        }
        writer.write(&signal).unwrap();
        writer.finish().unwrap();

        let before = std::fs::read(&path).unwrap();
        assert!(Mp3Writer::open(&path, 1, 128).is_err());
        assert_eq!(before, std::fs::read(&path).unwrap());

        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-f", "f32le", "-acodec", "pcm_f32le", "-"])
            .output()
            .unwrap();
        assert!(decoded.status.success());
        let samples: Vec<f32> = decoded
            .stdout
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect();
        assert!(samples.len() >= 48_000);
        assert!(samples[47_000..].iter().any(|value| value.abs() > 0.2));
        std::fs::remove_file(path).unwrap();
    }
}
