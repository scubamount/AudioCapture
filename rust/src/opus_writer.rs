//! Streaming Ogg Opus output using a statically linked libopus encoder.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use opus::{Application, Bitrate, Channels, Encoder};

const SAMPLE_RATE: u32 = 48_000;
const FRAME_SAMPLES: usize = 960;
const SERIAL: u32 = 0x6175_6469;

pub struct OpusWriter {
    encoder: Encoder,
    output: PacketWriter<'static, BufWriter<File>>,
    channels: usize,
    pre_skip: u16,
    buffer: Vec<f32>,
    input_frames: u64,
    encoded_frames: u64,
    pending: Option<(Vec<u8>, u64)>,
}

impl OpusWriter {
    pub fn open(path: &Path, channels: u16, bitrate: u32) -> Result<Self, String> {
        let channel_mode = match channels {
            1 => Channels::Mono,
            2 => Channels::Stereo,
            _ => return Err("Opus requires one or two channels".into()),
        };
        let bits = bitrate
            .checked_mul(1000)
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| "Invalid Opus bitrate".to_string())?;
        let mut encoder = Encoder::new(SAMPLE_RATE, channel_mode, Application::Audio)
            .map_err(|error| format!("Cannot initialize Opus: {error}"))?;
        encoder
            .set_bitrate(Bitrate::Bits(bits))
            .map_err(|error| format!("Cannot set Opus bitrate: {error}"))?;
        let pre_skip = u16::try_from(
            encoder
                .get_lookahead()
                .map_err(|error| format!("Cannot read Opus encoder delay: {error}"))?,
        )
        .map_err(|_| "Invalid Opus encoder delay".to_string())?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| format!("Cannot create {}: {error}", path.display()))?;
        let mut output = PacketWriter::new(BufWriter::new(file));
        let mut head = b"OpusHead".to_vec();
        head.push(1);
        head.push(channels as u8);
        head.extend_from_slice(&pre_skip.to_le_bytes());
        head.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes());
        head.push(0);
        output
            .write_packet(head, SERIAL, PacketWriteEndInfo::EndPage, 0)
            .map_err(|error| format!("Cannot write Opus header: {error}"))?;
        let vendor = b"AudioCapture";
        let mut tags = b"OpusTags".to_vec();
        tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        tags.extend_from_slice(vendor);
        tags.extend_from_slice(&0u32.to_le_bytes());
        output
            .write_packet(tags, SERIAL, PacketWriteEndInfo::EndPage, 0)
            .map_err(|error| format!("Cannot write Opus tags: {error}"))?;
        Ok(Self {
            encoder,
            output,
            channels: channels as usize,
            pre_skip,
            buffer: Vec::with_capacity(FRAME_SAMPLES * channels as usize),
            input_frames: 0,
            encoded_frames: 0,
            pending: None,
        })
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        if samples.len() % 2 != 0 {
            return Err("Stereo input has an incomplete frame".into());
        }
        for frame in samples.chunks_exact(2) {
            if self.channels == 1 {
                self.buffer.push((frame[0] + frame[1]) * 0.5);
            } else {
                self.buffer.extend_from_slice(frame);
            }
            self.input_frames = self
                .input_frames
                .checked_add(1)
                .ok_or_else(|| "Opus recording is too long".to_string())?;
            if self.buffer.len() == FRAME_SAMPLES * self.channels {
                self.encode_buffer()?;
            }
        }
        Ok(())
    }

    fn encode_buffer(&mut self) -> Result<(), String> {
        let mut packet = vec![0; 4000];
        let size = self
            .encoder
            .encode_float(&self.buffer, &mut packet)
            .map_err(|error| format!("Opus encoding failed: {error}"))?;
        packet.truncate(size);
        self.buffer.clear();
        self.encoded_frames += FRAME_SAMPLES as u64;
        if let Some((previous, granule)) = self.pending.replace((packet, self.encoded_frames)) {
            self.output
                .write_packet(previous, SERIAL, PacketWriteEndInfo::NormalPacket, granule)
                .map_err(|error| format!("Cannot write Opus data: {error}"))?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), String> {
        let final_granule = self
            .input_frames
            .checked_add(u64::from(self.pre_skip))
            .ok_or_else(|| "Opus recording is too long".to_string())?;
        while self.encoded_frames < final_granule || self.pending.is_none() {
            self.buffer.resize(FRAME_SAMPLES * self.channels, 0.0);
            self.encode_buffer()?;
        }
        let (packet, _) = self.pending.take().ok_or("Missing Opus final packet")?;
        self.output
            .write_packet(packet, SERIAL, PacketWriteEndInfo::EndStream, final_granule)
            .map_err(|error| format!("Cannot finish Opus stream: {error}"))?;
        self.output
            .inner_mut()
            .flush()
            .map_err(|error| format!("Cannot flush Opus stream: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    fn temporary_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "audio-capture-opus-{}-{}.opus",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn tone(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|frame| {
                let phase = frame as f32 * std::f32::consts::TAU * 440.0 / SAMPLE_RATE as f32;
                let sample = phase.sin() * 0.2;
                [sample, sample]
            })
            .collect()
    }

    #[test]
    fn output_is_decodable_and_preserves_sample_count() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            eprintln!("ffmpeg unavailable; external decoding check skipped");
            return;
        }
        for (frames, channels) in [(0, 1), (123, 1), (960, 2), (152_123, 1)] {
            let path = temporary_path();
            let mut writer = OpusWriter::open(&path, channels, 32).unwrap();
            for chunk in tone(frames).chunks(178) {
                writer.write(chunk).unwrap();
            }
            writer.finish().unwrap();
            let decoded = Command::new("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(&path)
                .args(["-f", "f32le", "-acodec", "pcm_f32le", "-"])
                .output()
                .unwrap();
            assert!(
                decoded.status.success(),
                "{}",
                String::from_utf8_lossy(&decoded.stderr)
            );
            assert_eq!(decoded.stdout.len() / (channels as usize * 4), frames);
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn speech_bitrate_is_smaller_and_existing_file_is_preserved() {
        let low = temporary_path();
        let high = temporary_path();
        let samples = tone(SAMPLE_RATE as usize * 10);
        for (path, bitrate, channels) in [(&low, 32, 1), (&high, 128, 2)] {
            let mut writer = OpusWriter::open(path, channels, bitrate).unwrap();
            writer.write(&samples).unwrap();
            writer.finish().unwrap();
        }
        let before = std::fs::read(&low).unwrap();
        assert!(OpusWriter::open(&low, 1, 32).is_err());
        assert_eq!(std::fs::read(&low).unwrap(), before);
        assert!(before.len() < std::fs::metadata(&high).unwrap().len() as usize);
        std::fs::remove_file(low).unwrap();
        std::fs::remove_file(high).unwrap();
    }
}
