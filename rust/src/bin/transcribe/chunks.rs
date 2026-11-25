//! Bounded, temporary WAV chunks for explicitly requested long-file jobs.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const RATE: u32 = 16_000;
const HEADER_BYTES: u64 = 44;
const MAX_DATA_BYTES: u64 = (u32::MAX as u64 - 36) & !1;
/// Input formats supported by the local conversion and duration probe.
///
/// Playlist and concat demuxers can open files that the user did not select.
/// Keep this list to self-contained recorder and common audio/video formats.
pub const MEDIA_DEMUXERS: &str = "wav,ogg,flac,mp3,aac,aiff,caf,matroska,mov,avi,asf";
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

struct TemporaryFile(PathBuf);

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct Decoder(Child);

impl Drop for Decoder {
    fn drop(&mut self) {
        // Waiting is required even when a request or a file operation fails.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn temporary_file(directory: &Path) -> Result<(TemporaryFile, File), String> {
    for _ in 0..100 {
        let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(
            ".audio-capture-chunk-{}-{id}.wav",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((TemporaryFile(path), file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Cannot create temporary audio chunk: {error}")),
        }
    }
    Err("Cannot find a unique temporary audio chunk name".into())
}

fn wav_header(data_bytes: u32) -> [u8; 44] {
    let mut header = [0u8; 44];
    header[..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    header[8..16].copy_from_slice(b"WAVEfmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes());
    header[20..22].copy_from_slice(&1u16.to_le_bytes());
    header[22..24].copy_from_slice(&1u16.to_le_bytes());
    header[24..28].copy_from_slice(&RATE.to_le_bytes());
    header[28..32].copy_from_slice(&(RATE * 2).to_le_bytes());
    header[32..34].copy_from_slice(&2u16.to_le_bytes());
    header[34..36].copy_from_slice(&16u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    header
}

fn for_each_pcm(
    mut pcm: impl Read,
    directory: &Path,
    limit_bytes: u64,
    mut callback: impl FnMut(&Path, u64, u64) -> Result<(), String>,
) -> Result<u64, String> {
    let capacity = limit_bytes
        .checked_sub(HEADER_BYTES)
        .map(|bytes| bytes.min(MAX_DATA_BYTES) & !1)
        .filter(|bytes| *bytes >= 2)
        .ok_or("Chunk limit must allow a WAV header and one PCM frame")?;
    let mut bytes = [0u8; 64 * 1024];
    let mut offset = 0u64;

    loop {
        // Read before creating the file so an exact boundary creates no empty part.
        let first_len = usize::try_from(capacity.min(bytes.len() as u64)).unwrap();
        let first = pcm
            .read(&mut bytes[..first_len])
            .map_err(|error| format!("Cannot read decoded audio: {error}"))?;
        if first == 0 {
            return Ok(offset);
        }
        let (temporary, mut file) = temporary_file(directory)?;
        file.write_all(&wav_header(0))
            .and_then(|_| file.write_all(&bytes[..first]))
            .map_err(|error| format!("Cannot write temporary audio chunk: {error}"))?;
        let mut written = first as u64;
        while written < capacity {
            let available = usize::try_from((capacity - written).min(bytes.len() as u64)).unwrap();
            let count = pcm
                .read(&mut bytes[..available])
                .map_err(|error| format!("Cannot read decoded audio: {error}"))?;
            if count == 0 {
                break;
            }
            file.write_all(&bytes[..count])
                .map_err(|error| format!("Cannot write temporary audio chunk: {error}"))?;
            written += count as u64;
        }
        if written % 2 != 0 {
            return Err("Decoder returned an incomplete 16-bit PCM frame".into());
        }
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.write_all(&wav_header(written as u32)))
            .and_then(|_| file.flush())
            .map_err(|error| format!("Cannot finalize temporary audio chunk: {error}"))?;
        drop(file);
        let frames = written / 2;
        callback(&temporary.0, offset, frames)?;
        offset = offset
            .checked_add(frames)
            .ok_or("Audio duration is too long")?;
        fs::remove_file(&temporary.0)
            .map_err(|error| format!("Cannot remove temporary audio chunk: {error}"))?;
        // Its drop is harmless after the explicit removal.
    }
}

/// Decode once and pass each bounded PCM WAV to the callback.
///
/// Offsets and lengths count mono frames at 16 kHz. Each path exists only
/// during its callback. A callback can succeed before the decoder later fails,
/// so callers must finalize the aggregate only after this function succeeds.
pub fn for_each_wav(
    input: &Path,
    job_dir: &Path,
    limit_bytes: u64,
    callback: impl FnMut(&Path, u64, u64) -> Result<(), String>,
) -> Result<(), String> {
    if limit_bytes < HEADER_BYTES + 2 {
        return Err("Chunk limit must allow a WAV header and one PCM frame".into());
    }
    let mut decoder = Decoder(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-protocol_whitelist",
                "file,pipe",
                "-format_whitelist",
                MEDIA_DEMUXERS,
                "-i",
            ])
            .arg(input)
            .args([
                "-map",
                "0:a:0",
                "-ac",
                "1",
                "-ar",
                "16000",
                "-c:a",
                "pcm_s16le",
                "-f",
                "s16le",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Cannot start ffmpeg for audio conversion: {error}"))?,
    );
    let stdout = decoder
        .0
        .stdout
        .take()
        .ok_or("Cannot open ffmpeg audio output")?;
    let frames = for_each_pcm(BufReader::new(stdout), job_dir, limit_bytes, callback)?;
    let status = decoder
        .0
        .wait()
        .map_err(|error| format!("Cannot wait for ffmpeg: {error}"))?;
    if !status.success() {
        return Err(format!("ffmpeg could not decode the input ({status})"));
    }
    if frames == 0 {
        return Err("The input contains no audio frames".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("audio-chunks-{}-{id}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn splits_with_exact_offsets_and_no_empty_tail() {
        let directory = TestDir::new();
        let pcm: Vec<u8> = (0..10).collect();
        let mut chunks = Vec::new();
        let total = for_each_pcm(
            Cursor::new(&pcm),
            &directory.0,
            48,
            |path, offset, count| {
                let data = fs::read(path).unwrap();
                assert!(data.len() <= 48);
                assert_eq!(&data[..4], b"RIFF");
                assert_eq!(
                    u32::from_le_bytes(data[40..44].try_into().unwrap()) as usize,
                    data.len() - 44
                );
                chunks.push((offset, count, data[44..].to_vec()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(total, 5);
        assert_eq!(
            chunks
                .iter()
                .map(|(offset, count, _)| (*offset, *count))
                .collect::<Vec<_>>(),
            [(0, 2), (2, 2), (4, 1)]
        );
        assert_eq!(
            chunks
                .into_iter()
                .flat_map(|(_, _, data)| data)
                .collect::<Vec<_>>(),
            pcm
        );
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }

    #[test]
    fn callback_failure_and_invalid_frames_remove_temporary_files() {
        let directory = TestDir::new();
        assert!(
            for_each_pcm(
                Cursor::new([1u8, 2, 3, 4]),
                &directory.0,
                48,
                |_, _, _| Err("cancelled".into())
            )
            .is_err()
        );
        assert!(
            for_each_pcm(Cursor::new([1u8, 2, 3]), &directory.0, 48, |_, _, _| Ok(())).is_err()
        );
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 0);
    }

    #[test]
    fn decoder_failure_reports_error_without_leaving_chunks() {
        let directory = TestDir::new();
        let input = directory.0.join("invalid.wav");
        fs::write(&input, b"not a wave file").unwrap();
        let mut calls = 0;
        let result = for_each_wav(&input, &directory.0, 48, |_, _, _| {
            calls += 1;
            Ok(())
        });
        assert!(result.unwrap_err().contains("ffmpeg could not decode"));
        assert_eq!(calls, 0);
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn rejects_playlist_disguised_as_wave_without_callbacks() {
        let directory = TestDir::new();
        let input = directory.0.join("invalid.wav");
        let target = directory.0.join("other.wav");
        let mut target_file = File::create(&target).unwrap();
        target_file.write_all(&wav_header(4)).unwrap();
        target_file.write_all(&[0, 0, 0, 0]).unwrap();
        drop(target_file);
        fs::write(&input, "ffconcat version 1.0\nfile 'other.wav'\n").unwrap();
        let mut calls = 0;
        assert!(
            for_each_wav(&input, &directory.0, 48, |_, _, _| {
                calls += 1;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(calls, 0);
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 2);
    }

    #[test]
    fn ffmpeg_decodes_an_oversized_input_into_bounded_wavs() {
        let directory = TestDir::new();
        let input = directory.0.join("input.wav");
        let pcm: Vec<u8> = (0..10).collect();
        let mut file = File::create(&input).unwrap();
        file.write_all(&wav_header(pcm.len() as u32)).unwrap();
        file.write_all(&pcm).unwrap();
        drop(file);
        let mut chunks = Vec::new();
        for_each_wav(&input, &directory.0, 48, |path, offset, count| {
            let data = fs::read(path).unwrap();
            assert!(data.len() <= 48);
            chunks.push((offset, count, data[44..].to_vec()));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            chunks
                .iter()
                .map(|(offset, count, _)| (*offset, *count))
                .collect::<Vec<_>>(),
            [(0, 2), (2, 2), (4, 1)]
        );
        assert_eq!(
            chunks
                .into_iter()
                .flat_map(|(_, _, data)| data)
                .collect::<Vec<_>>(),
            pcm
        );
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn ffmpeg_accepts_ogg_opus_with_input_restrictions() {
        let directory = TestDir::new();
        let input = directory.0.join("input.opus");
        let status = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=16000:cl=mono",
                "-t",
                "0.1",
                "-c:a",
                "libopus",
                "-n",
            ])
            .arg(&input)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("ffmpeg is needed for the Opus chunk test");
        assert!(status.success(), "ffmpeg must support the libopus encoder");
        let mut expected_offset = 0;
        for_each_wav(&input, &directory.0, 1024, |path, offset, count| {
            assert_eq!(offset, expected_offset);
            assert!(fs::metadata(path).unwrap().len() <= 1024);
            expected_offset += count;
            Ok(())
        })
        .unwrap();
        assert_eq!(expected_offset, 1600);
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }
}
