//! Streaming audio output for WAV, MP3, Ogg Opus, and FLAC.
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub struct Writer {
    path: PathBuf,
    kind: Kind,
}

enum Kind {
    Wav(WavWriter),
    Opus(Option<crate::opus_writer::OpusWriter>),
    Flac(Option<crate::flac_writer::FlacWriter>),
    Mp3(Option<crate::mp3_writer::Mp3Writer>),
}

struct WavWriter {
    file: BufWriter<File>,
    path: PathBuf,
    part: u32,
    max_file_size: u32,
    rate: u32,
    channels: u16,
    // Input and output sample intervals use an integer common timebase. This
    // keeps conversion stable across buffer boundaries and long sessions.
    phase: u32,
    sums: [f64; 2],
    data_bytes: u32,
    filter: Vec<f64>,
    history: [[f64; 2]; 63],
    history_index: usize,
}

impl Writer {
    pub fn open(
        path: &Path,
        format: &str,
        rate: u32,
        channels: u16,
        bitrate: u32,
        flac_level: u8,
    ) -> Result<Self, String> {
        if ![1, 2].contains(&channels) {
            return Err("Audio must have one or two channels".into());
        }
        if ![16_000, 44_100, 48_000].contains(&rate) {
            return Err(format!("Unsupported sample rate: {rate} Hz"));
        }
        let kind = if format == "wav" {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
            let filter = if rate == 48_000 {
                Vec::new()
            } else {
                let cutoff = 0.45 * rate as f64 / 48_000.0;
                let mut coefficients: Vec<f64> = (0..63)
                    .map(|index| {
                        let offset = index as f64 - 31.0;
                        let sinc = if offset == 0.0 {
                            2.0 * cutoff
                        } else {
                            (2.0 * std::f64::consts::PI * cutoff * offset).sin()
                                / (std::f64::consts::PI * offset)
                        };
                        let window = 0.42
                            - 0.5 * (2.0 * std::f64::consts::PI * index as f64 / 62.0).cos()
                            + 0.08 * (4.0 * std::f64::consts::PI * index as f64 / 62.0).cos();
                        sinc * window
                    })
                    .collect();
                let gain: f64 = coefficients.iter().sum();
                for coefficient in &mut coefficients {
                    *coefficient /= gain;
                }
                coefficients
            };
            let mut wav = WavWriter {
                file: BufWriter::new(file),
                path: path.to_owned(),
                part: 1,
                max_file_size: 4_000_000_000,
                rate,
                channels,
                phase: 0,
                sums: [0.0; 2],
                data_bytes: 0,
                filter,
                history: [[0.0; 2]; 63],
                history_index: 0,
            };
            wav.header()
                .map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
            Kind::Wav(wav)
        } else if format == "opus" {
            if rate != 48_000 {
                return Err("Opus output uses a 48000 Hz stream".into());
            }
            Kind::Opus(Some(crate::opus_writer::OpusWriter::open(
                path, channels, bitrate,
            )?))
        } else if format == "flac" {
            if rate != 48_000 {
                return Err("FLAC output uses a 48000 Hz stream".into());
            }
            Kind::Flac(Some(crate::flac_writer::FlacWriter::open(
                path, channels, flac_level,
            )?))
        } else if format == "mp3" {
            if rate != 48_000 {
                return Err("MP3 output uses a 48000 Hz stream".into());
            }
            Kind::Mp3(Some(crate::mp3_writer::Mp3Writer::open(
                path, channels, bitrate,
            )?))
        } else {
            return Err(format!("Unsupported audio format: {format}"));
        };
        Ok(Self {
            path: path.to_owned(),
            kind,
        })
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        if samples.len() % 2 != 0 {
            return Err("Input must contain complete stereo frames".into());
        }
        let result = match &mut self.kind {
            Kind::Wav(wav) => wav.write(samples),
            Kind::Opus(opus) => {
                return opus
                    .as_mut()
                    .ok_or("Opus encoder is closed")?
                    .write(samples);
            }
            Kind::Flac(flac) => {
                return flac
                    .as_mut()
                    .ok_or("FLAC encoder is closed")?
                    .write(samples);
            }
            Kind::Mp3(mp3) => return mp3.as_mut().ok_or("MP3 encoder is closed")?.write(samples),
        };
        result.map_err(|e| format!("Cannot encode {}: {e}", self.path.display()))
    }

    pub fn finish(mut self) -> Result<(), String> {
        match &mut self.kind {
            Kind::Wav(wav) => wav
                .header()
                .and_then(|()| wav.file.flush())
                .map_err(|e| format!("Cannot finalize {}: {e}", self.path.display())),
            Kind::Opus(opus) => opus.take().ok_or("Opus encoder is closed")?.finish(),
            Kind::Flac(flac) => flac.take().ok_or("FLAC encoder is closed")?.finish(),
            Kind::Mp3(mp3) => mp3.take().ok_or("MP3 encoder is closed")?.finish(),
        }
    }
}

impl WavWriter {
    fn header(&mut self) -> io::Result<()> {
        let file = &mut self.file;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(b"RIFF")?;
        file.write_all(&(36 + self.data_bytes).to_le_bytes())?;
        file.write_all(b"WAVEfmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&self.channels.to_le_bytes())?;
        file.write_all(&self.rate.to_le_bytes())?;
        file.write_all(&(self.rate * self.channels as u32 * 2).to_le_bytes())?;
        file.write_all(&(self.channels * 2).to_le_bytes())?;
        file.write_all(&16u16.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&self.data_bytes.to_le_bytes())?;
        file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    fn write(&mut self, samples: &[f32]) -> io::Result<()> {
        let mut output = Vec::with_capacity(samples.len() * 2);
        for frame in samples.chunks_exact(2) {
            let mut source = if self.channels == 1 {
                [(frame[0] as f64 + frame[1] as f64) * 0.5, 0.0]
            } else {
                [frame[0] as f64, frame[1] as f64]
            };
            if !self.filter.is_empty() {
                self.history[self.history_index] = source;
                let mut filtered = [0.0; 2];
                for (offset, coefficient) in self.filter.iter().enumerate() {
                    let previous = self.history[(self.history_index + 63 - offset) % 63];
                    filtered[0] += previous[0] * coefficient;
                    filtered[1] += previous[1] * coefficient;
                }
                self.history_index = (self.history_index + 1) % 63;
                source = filtered;
            }
            let mut remaining = self.rate;
            while remaining > 0 {
                let portion = remaining.min(48_000 - self.phase);
                for (sum, sample) in self.sums.iter_mut().zip(source) {
                    *sum += sample * portion as f64;
                }
                self.phase += portion;
                remaining -= portion;
                if self.phase == 48_000 {
                    for sum in self.sums.iter().take(self.channels as usize) {
                        let value = (sum / 48_000.0).clamp(-1.0, 1.0);
                        output.extend_from_slice(&((value * 32767.0).round() as i16).to_le_bytes());
                    }
                    self.phase = 0;
                    self.sums = [0.0; 2];
                }
            }
        }
        let frame_bytes = usize::from(self.channels) * 2;
        let max_data_bytes =
            ((self.max_file_size.saturating_sub(44)) as usize / frame_bytes) * frame_bytes;
        if max_data_bytes == 0 {
            return Err(io::Error::other("WAV size limit does not fit one frame"));
        }
        let mut offset = 0;
        while offset < output.len() {
            if self.data_bytes as usize == max_data_bytes {
                self.next_part()?;
            }
            let length = (output.len() - offset).min(max_data_bytes - self.data_bytes as usize);
            self.file.write_all(&output[offset..offset + length])?;
            self.data_bytes += length as u32;
            offset += length;
        }
        Ok(())
    }

    fn next_part(&mut self) -> io::Result<()> {
        self.header()?;
        self.file.flush()?;
        let next = self
            .part
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Too many WAV parts"))?;
        let mut name = self
            .path
            .file_stem()
            .ok_or_else(|| io::Error::other("WAV path has no file name"))?
            .to_os_string();
        name.push(format!("_part{next}"));
        if let Some(extension) = self.path.extension() {
            name.push(".");
            name.push(extension);
        }
        let next_path = self.path.with_file_name(name);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&next_path)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("Cannot create {}: {error}", next_path.display()),
                )
            })?;
        self.file = BufWriter::new(file);
        self.part = next;
        self.data_bytes = 0;
        self.header()
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if let Kind::Wav(wav) = &mut self.kind {
            let _ = wav.header();
            let _ = wav.file.flush();
        }
        if let Kind::Opus(opus) = &mut self.kind {
            if let Some(opus) = opus.take() {
                let _ = opus.finish();
            }
        }
        if let Kind::Flac(flac) = &mut self.kind {
            if let Some(flac) = flac.take() {
                let _ = flac.finish();
            }
        }
        if let Kind::Mp3(mp3) = &mut self.kind {
            if let Some(mp3) = mp3.take() {
                let _ = mp3.finish();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn destination(extension: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "audio-capture-test-{}-{stamp}.{extension}",
            std::process::id()
        ))
    }

    #[test]
    fn wav_rate_and_channels_are_correct_across_write_boundaries() {
        for rate in [16_000, 44_100, 48_000] {
            let path = destination("wav");
            let mut writer = Writer::open(&path, "wav", rate, 1, 0, 0).unwrap();
            let samples = vec![0.25f32; 48_000 * 2];
            for part in samples.chunks(246) {
                writer.write(part).unwrap();
            }
            writer.finish().unwrap();
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), rate);
            assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 1);
            assert_eq!(
                u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
                rate * 2
            );
            assert_eq!(bytes.len(), 44 + rate as usize * 2);
            assert!((i16::from_le_bytes(bytes[2044..2046].try_into().unwrap()) - 8192).abs() <= 1);
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn existing_output_is_never_overwritten() {
        let path = destination("wav");
        std::fs::write(&path, b"existing").unwrap();
        assert!(Writer::open(&path, "wav", 48_000, 2, 0, 0).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"existing");
        std::fs::remove_file(path).unwrap();
    }

    fn pcm_payload(wav: &[u8]) -> &[u8] {
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        let mut offset = 12;
        while offset + 8 <= wav.len() {
            let size = u32::from_le_bytes(wav[offset + 4..offset + 8].try_into().unwrap()) as usize;
            let start = offset + 8;
            if &wav[offset..offset + 4] == b"fmt " {
                assert_eq!(
                    u16::from_le_bytes(wav[start..start + 2].try_into().unwrap()),
                    1
                );
                assert_eq!(
                    u16::from_le_bytes(wav[start + 2..start + 4].try_into().unwrap()),
                    2
                );
                assert_eq!(
                    u32::from_le_bytes(wav[start + 4..start + 8].try_into().unwrap()),
                    48_000
                );
                assert_eq!(
                    u16::from_le_bytes(wav[start + 14..start + 16].try_into().unwrap()),
                    16
                );
            }
            if &wav[offset..offset + 4] == b"data" {
                return &wav[start..start + size];
            }
            offset = start + size + (size & 1);
        }
        panic!("missing WAV data chunk");
    }

    #[test]
    fn pcm_matches_original_writer_output_across_chunks() {
        let original = include_bytes!("../tests/fixtures/upstream-wav-pcm16.wav");
        let path = destination("wav");
        let mut writer = Writer::open(&path, "wav", 48_000, 2, 0, 0).unwrap();
        let samples = [
            0.0, 0.0, 0.25, -0.25, 1.0, -1.0, -0.25, 0.25, 0.5, -0.5, 0.0, 0.0,
        ];
        writer.write(&samples[..4]).unwrap();
        writer.write(&samples[4..]).unwrap();
        writer.finish().unwrap();
        let output = std::fs::read(&path).unwrap();
        assert_eq!(pcm_payload(&output), pcm_payload(original));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn wav_rollover_keeps_frames_and_resampler_state() {
        let unsplit_path = destination("wav");
        let split_path = destination("wav");
        let mut unsplit = Writer::open(&unsplit_path, "wav", 44_100, 2, 0, 0).unwrap();
        let mut split = Writer::open(&split_path, "wav", 44_100, 2, 0, 0).unwrap();
        if let Kind::Wav(wav) = &mut split.kind {
            wav.max_file_size = 100;
        }
        let samples: Vec<f32> = (0..100)
            .flat_map(|frame| {
                let sample = ((frame as f32) * 0.071).sin() * 0.3;
                [sample, -sample]
            })
            .collect();
        for chunk in samples.chunks(14) {
            unsplit.write(chunk).unwrap();
            split.write(chunk).unwrap();
        }
        unsplit.finish().unwrap();
        split.finish().unwrap();
        let full = std::fs::read(&unsplit_path).unwrap();
        let mut joined = Vec::new();
        let mut part = 1;
        loop {
            let path = if part == 1 {
                split_path.clone()
            } else {
                split_path.with_file_name(format!(
                    "{}_part{part}.wav",
                    split_path.file_stem().unwrap().to_string_lossy()
                ))
            };
            if !path.exists() {
                break;
            }
            let bytes = std::fs::read(&path).unwrap();
            assert!(bytes.len() <= 100);
            assert_eq!((bytes.len() - 44) % 4, 0);
            assert_eq!(
                u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize,
                bytes.len() - 44
            );
            joined.extend_from_slice(&bytes[44..]);
            std::fs::remove_file(path).unwrap();
            part += 1;
        }
        assert!(part > 3);
        assert_eq!(&full[44..], joined);
        std::fs::remove_file(unsplit_path).unwrap();
    }

    #[test]
    fn wav_rollover_collision_preserves_existing_and_completed_part() {
        let path = destination("wav");
        let part2 = path.with_file_name(format!(
            "{}_part2.wav",
            path.file_stem().unwrap().to_string_lossy()
        ));
        std::fs::write(&part2, b"existing").unwrap();
        let mut writer = Writer::open(&path, "wav", 48_000, 2, 0, 0).unwrap();
        if let Kind::Wav(wav) = &mut writer.kind {
            wav.max_file_size = 52;
        }
        assert!(writer.write(&[0.25; 6]).is_err());
        writer.finish().unwrap();
        assert_eq!(std::fs::read(&part2).unwrap(), b"existing");
        let first = std::fs::read(&path).unwrap();
        assert_eq!(first.len(), 52);
        assert_eq!(u32::from_le_bytes(first[40..44].try_into().unwrap()), 8);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(part2).unwrap();
    }

    #[test]
    fn downsampling_rejects_high_frequency_aliasing() {
        let path = destination("wav");
        let mut writer = Writer::open(&path, "wav", 16_000, 1, 0, 0).unwrap();
        let samples: Vec<_> = (0..48_000)
            .flat_map(|i| {
                let sample = ((i as f32) * std::f32::consts::TAU * 12_000.0 / 48_000.0).sin() * 0.5;
                [sample, sample]
            })
            .collect();
        writer.write(&samples).unwrap();
        writer.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let peak = bytes[2044..]
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs())
            .max()
            .unwrap();
        assert!(peak < 500, "12 kHz rejection was insufficient: {peak}");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn native_codecs_produce_readable_files_when_ffmpeg_is_available() {
        if Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            return;
        }
        for format in ["mp3", "opus", "flac"] {
            let path = destination(format);
            let mut writer = Writer::open(
                &path,
                format,
                48_000,
                1,
                if format == "mp3" { 128 } else { 32 },
                5,
            )
            .unwrap();
            let samples: Vec<_> = (0..48_000)
                .flat_map(|i| {
                    let sample =
                        ((i as f32) * std::f32::consts::TAU * 440.0 / 48_000.0).sin() * 0.25;
                    [sample, sample]
                })
                .collect();
            writer.write(&samples).unwrap();
            writer.finish().unwrap();
            let status = Command::new("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(&path)
                .args(["-f", "null", "-"])
                .status()
                .unwrap();
            assert!(status.success(), "{format} did not decode");
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    #[ignore = "requires AUDIOCAPTURE_BENCH_F32 with generated 48 kHz stereo float audio"]
    fn compare_speech_recording_sizes() {
        let path = std::env::var_os("AUDIOCAPTURE_BENCH_F32").expect("benchmark input");
        let bytes = std::fs::read(path).unwrap();
        let samples: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|sample| f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]))
            .collect();
        let mut results = Vec::new();
        for (format, rate, channels, bitrate) in [
            ("wav", 48_000, 2, 0),
            ("wav", 16_000, 1, 0),
            ("opus", 48_000, 2, 128),
            ("opus", 48_000, 1, 32),
            ("opus", 48_000, 1, 24),
        ] {
            let path = destination(format);
            let mut writer = Writer::open(&path, format, rate, channels, bitrate, 5).unwrap();
            for part in samples.chunks(960) {
                writer.write(part).unwrap();
            }
            writer.finish().unwrap();
            let size = std::fs::metadata(&path).unwrap().len();
            println!("{format} {rate} Hz {channels} channels {bitrate} kbps: {size} bytes");
            results.push(size);
            std::fs::remove_file(path).unwrap();
        }
        assert!(results[1] < results[0]);
        assert!(results[3] < results[2]);
        assert!(results[4] < results[2]);
    }
}
