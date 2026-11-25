//! Versioned, local framing for the recorder's inherited worker pipe.
use crate::record::LiveTap;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufWriter, Read, Write},
    process::ChildStdin,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAGIC: &[u8; 4] = b"ACL1";
const BLOCK: usize = 480;
const BATCH: usize = 4_800;
pub const MAX_LIVE_SOURCES: usize = 16;
/// Flip these only when the corresponding profile passes its release gates.
pub const QUALIFIED_LIVE_GATE: bool = false;
pub const QUALIFIED_LIVE_SPEAKERS: bool = false;
const MAX_CONTROL: usize = 64 * 1024;
const AUDIO_SLOTS: usize = 512;
const CONTROL_RESERVE: usize = 128;
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub enum Frame {
    Start {
        session: String,
        interval: String,
        first: u64,
        sources: Vec<String>,
    },
    #[serde(skip)]
    Audio {
        source: u16,
        first: u64,
        pcm: Vec<i16>,
    },
    Gap {
        source: u16,
        first: u64,
        end: u64,
        #[serde(default)]
        reason: GapReason,
    },
    Speakers {
        enabled: bool,
        frame: u64,
    },
    Timing {
        source: u16,
        frame: u64,
    },
    Recording {
        id: String,
        start: u64,
        end: u64,
    },
    End {
        end: u64,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum GapReason {
    #[default]
    SourceUnavailable,
    TapOverflow,
}

impl GapReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SourceUnavailable => "Capture source unavailable",
            Self::TapOverflow => "Live audio queue full",
        }
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn header(out: &mut impl Write, kind: u8, length: usize) -> io::Result<()> {
    out.write_all(MAGIC)?;
    out.write_all(&[kind])?;
    out.write_all(&(length as u32).to_le_bytes())
}

fn write_audio(out: &mut impl Write, source: u16, first: u64, pcm: &[i16]) -> io::Result<()> {
    if pcm.is_empty() || pcm.len() > BATCH || first.checked_add(pcm.len() as u64).is_none() {
        return Err(invalid("Invalid live audio range"));
    }
    header(out, 1, 10 + pcm.len() * 2)?;
    out.write_all(&source.to_le_bytes())?;
    out.write_all(&first.to_le_bytes())?;
    let mut bytes = [0u8; BATCH * 2];
    for (target, sample) in bytes.chunks_exact_mut(2).zip(pcm) {
        target.copy_from_slice(&sample.to_le_bytes());
    }
    out.write_all(&bytes[..pcm.len() * 2])
}

pub fn write_frame(out: &mut impl Write, frame: &Frame) -> io::Result<()> {
    if let Frame::Audio { source, first, pcm } = frame {
        return write_audio(out, *source, *first, pcm);
    }
    let bytes = serde_json::to_vec(frame)?;
    if bytes.len() > MAX_CONTROL {
        return Err(invalid("Live metadata exceeds the protocol limit"));
    }
    header(out, 0, bytes.len())?;
    out.write_all(&bytes)
}

pub fn read_frame(reader: &mut impl Read) -> io::Result<Option<Frame>> {
    let mut prefix = [0u8; 9];
    loop {
        match reader.read(&mut prefix[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    reader.read_exact(&mut prefix[1..])?;
    if &prefix[..4] != MAGIC {
        return Err(invalid("Unsupported live pipe protocol"));
    }
    let size = u32::from_le_bytes(prefix[5..9].try_into().unwrap()) as usize;
    match prefix[4] {
        0 if size > 0 && size <= MAX_CONTROL => {
            let mut bytes = vec![0; size];
            reader.read_exact(&mut bytes)?;
            Ok(Some(serde_json::from_slice(&bytes)?))
        }
        1 if (12..=10 + BATCH * 2).contains(&size) && size % 2 == 0 => {
            let mut position = [0; 10];
            reader.read_exact(&mut position)?;
            let source = u16::from_le_bytes(position[..2].try_into().unwrap());
            let first = u64::from_le_bytes(position[2..].try_into().unwrap());
            let count = (size - 10) / 2;
            first
                .checked_add(count as u64)
                .ok_or_else(|| invalid("Live audio time overflow"))?;
            let mut bytes = [0u8; BATCH * 2];
            reader.read_exact(&mut bytes[..count * 2])?;
            let pcm = bytes[..count * 2]
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            Ok(Some(Frame::Audio { source, first, pcm }))
        }
        _ => Err(invalid("Invalid live frame type or size")),
    }
}

pub fn new_interval_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "live-{stamp:x}-{:x}-{:x}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

// Fixed inline storage avoids allocating audio on the shared capture thread.
// A single producer reserves some queue slots for control events.
enum Packet {
    Audio {
        source: u16,
        first: u64,
        len: u16,
        samples: [f32; BLOCK * 2],
    },
    Missing {
        source: u16,
        first: u64,
        end: u64,
    },
    Control(Frame),
}

struct Tap {
    id: String,
    sources: Vec<String>,
    missing: Vec<Option<(u64, u64)>>,
    tx: Option<mpsc::SyncSender<Packet>>,
    outstanding: Arc<AtomicUsize>,
    stop: Arc<OnceLock<u64>>,
}

impl Tap {
    fn send_data(&self, packet: Packet) {
        let Some(tx) = self.tx.as_ref() else { return };
        if self.outstanding.load(Ordering::Relaxed) < AUDIO_SLOTS {
            self.outstanding.fetch_add(1, Ordering::Relaxed);
            if tx.try_send(packet).is_err() {
                self.outstanding.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
    fn send_gap(&mut self, source: usize) {
        if let Some((first, end)) = self.missing[source].take() {
            self.send_data(Packet::Missing {
                source: source as u16,
                first,
                end,
            });
        }
    }
    fn send_gaps(&mut self) {
        for source in 0..self.missing.len() {
            self.send_gap(source);
        }
    }
    fn control(&mut self, frame: Frame) {
        if self
            .tx
            .as_ref()
            .is_some_and(|tx| tx.try_send(Packet::Control(frame)).is_err())
        {
            // EOF before End marks the stream incomplete if even the reserve is exhausted.
            self.tx.take();
        }
    }
}

impl LiveTap for Tap {
    fn interval_id(&self) -> &str {
        &self.id
    }
    fn start(&mut self, session: &str, frame: u64) {
        let sources = std::mem::take(&mut self.sources);
        self.control(Frame::Start {
            session: session.to_owned(),
            interval: self.id.clone(),
            first: frame,
            sources,
        });
    }
    fn audio(&mut self, source: usize, first: u64, stereo: &[f32]) {
        if source >= self.missing.len() || self.tx.is_none() {
            return;
        }
        self.send_gap(source);
        for (i, part) in stereo.chunks(BLOCK * 2).enumerate() {
            let len = part.len() / 2;
            if len == 0 || self.outstanding.load(Ordering::Relaxed) >= AUDIO_SLOTS {
                return;
            }
            let mut samples = [0.0; BLOCK * 2];
            samples[..len * 2].copy_from_slice(&part[..len * 2]);
            let packet = Packet::Audio {
                source: source as u16,
                first: first + (i * BLOCK) as u64,
                len: len as u16,
                samples,
            };
            self.send_data(packet);
        }
    }
    fn gap(&mut self, source: usize, first: u64, end: u64) {
        if source >= self.missing.len() || end <= first {
            return;
        }
        if self.missing[source].is_some_and(|(_, previous)| previous != first) {
            self.send_gap(source);
        }
        let range = self.missing[source].get_or_insert((first, end));
        range.1 = end;
        if end.saturating_sub(range.0) >= 48_000 {
            self.send_gap(source);
        }
    }
    fn speakers(&mut self, enabled: bool, frame: u64) {
        self.send_gaps();
        self.control(Frame::Speakers { enabled, frame });
    }
    fn uncertain(&mut self, source: usize, observed_at: u64) {
        if source < self.missing.len() {
            self.send_gap(source);
            self.control(Frame::Timing {
                source: source as u16,
                frame: observed_at,
            });
        }
    }
    fn recording(&mut self, recording_id: &str, start: u64, end: u64) {
        self.control(Frame::Recording {
            id: recording_id.to_owned(),
            start,
            end,
        });
    }
    fn finish(&mut self, end: u64) {
        self.send_gaps();
        if self.tx.is_some() {
            let _ = self.stop.set(end);
        }
        self.tx.take();
    }
}

struct SourceBuffer {
    next: u64,
    pcm: Vec<i16>,
}

impl SourceBuffer {
    fn flush(&mut self, writer: &mut impl Write, source: u16) -> io::Result<()> {
        if !self.pcm.is_empty() {
            write_audio(writer, source, self.next - self.pcm.len() as u64, &self.pcm)?;
            self.pcm.clear();
        }
        Ok(())
    }
    fn gap_until(
        &mut self,
        writer: &mut impl Write,
        source: u16,
        frame: u64,
        reason: GapReason,
    ) -> io::Result<()> {
        if frame > self.next {
            self.flush(writer, source)?;
            write_frame(
                writer,
                &Frame::Gap {
                    source,
                    first: self.next,
                    end: frame,
                    reason,
                },
            )?;
            self.next = frame;
        }
        Ok(())
    }
    fn add(
        &mut self,
        writer: &mut impl Write,
        source: u16,
        first: u64,
        stereo: &[f32],
    ) -> io::Result<()> {
        self.gap_until(writer, source, first, GapReason::TapOverflow)?;
        let skip = usize::try_from(self.next - first).unwrap_or(usize::MAX);
        for pair in stereo.chunks_exact(2).skip(skip) {
            let mono = (pair[0] + pair[1]) * 0.5;
            self.pcm
                .push((mono * 32768.0).round().clamp(-32768.0, 32767.0) as i16);
            self.next += 1;
            if self.pcm.len() == BATCH {
                self.flush(writer, source)?;
            }
        }
        Ok(())
    }
}

fn flush_sources(out: &mut impl Write, buffers: &mut [SourceBuffer]) -> io::Result<()> {
    for (i, buf) in buffers.iter_mut().enumerate() {
        buf.flush(out, i as u16)?;
    }
    out.flush()
}

fn relay(
    writer: impl Write,
    rx: mpsc::Receiver<Packet>,
    outstanding: &AtomicUsize,
    stop: &OnceLock<u64>,
) -> io::Result<()> {
    let mut out = BufWriter::with_capacity(32 * 1024, writer);
    let mut buffers = Vec::new();
    let mut deadline = Instant::now() + FLUSH_INTERVAL;
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Packet::Control(frame)) => {
                flush_sources(&mut out, &mut buffers)?;
                if let Frame::Start { first, sources, .. } = &frame {
                    buffers = sources
                        .iter()
                        .map(|_| SourceBuffer {
                            next: *first,
                            pcm: Vec::with_capacity(BATCH),
                        })
                        .collect();
                }
                write_frame(&mut out, &frame)?;
                out.flush()?;
            }
            Ok(Packet::Audio {
                source,
                first,
                len,
                samples,
            }) => {
                outstanding.fetch_sub(1, Ordering::Relaxed);
                let buffer = buffers
                    .get_mut(source as usize)
                    .ok_or_else(|| invalid("Unknown live source"))?;
                buffer.add(&mut out, source, first, &samples[..len as usize * 2])?;
            }
            Ok(Packet::Missing { source, first, end }) => {
                outstanding.fetch_sub(1, Ordering::Relaxed);
                let buffer = buffers
                    .get_mut(source as usize)
                    .ok_or_else(|| invalid("Unknown live source"))?;
                buffer.gap_until(&mut out, source, first, GapReason::TapOverflow)?;
                buffer.gap_until(&mut out, source, end, GapReason::SourceUnavailable)?;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if Instant::now() >= deadline {
            flush_sources(&mut out, &mut buffers)?;
            deadline = Instant::now() + FLUSH_INTERVAL;
        }
    }
    flush_sources(&mut out, &mut buffers)?;
    if let Some(&end) = stop.get() {
        for (i, buffer) in buffers.iter_mut().enumerate() {
            buffer.gap_until(&mut out, i as u16, end, GapReason::TapOverflow)?;
        }
        write_frame(&mut out, &Frame::End { end })?;
    }
    out.flush()
}

pub fn launch_relay(
    stdin: ChildStdin,
    interval: String,
    sources: Vec<String>,
) -> io::Result<Box<dyn LiveTap>> {
    if sources.is_empty()
        || sources.len() > MAX_LIVE_SOURCES
        || sources.iter().any(|s| s.is_empty() || s.len() > 256)
    {
        return Err(invalid("Choose between one and 16 named live sources"));
    }
    let (tx, rx) = mpsc::sync_channel(AUDIO_SLOTS + CONTROL_RESERVE);
    let outstanding = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(OnceLock::new());
    let missing = vec![None; sources.len()];
    let tap = Tap {
        id: interval,
        sources,
        missing,
        tx: Some(tx),
        outstanding: outstanding.clone(),
        stop: stop.clone(),
    };
    thread::Builder::new()
        .name("live-audio-relay".into())
        .spawn(move || {
            // The worker detects an interrupted stream from EOF before End.
            let _ = relay(stdin, rx, &outstanding, &stop);
        })?;
    Ok(Box::new(tap))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn preserves_exact_sample_positions_and_signed_pcm() {
        let input = Frame::Audio {
            source: 1,
            first: 48_003,
            pcm: vec![-32768, 0, 32767],
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &input).unwrap();
        let mut reader = Cursor::new(bytes);
        assert_eq!(read_frame(&mut reader).unwrap(), Some(input));
        assert_eq!(read_frame(&mut reader).unwrap(), None);
    }

    #[test]
    fn distinguishes_clean_eof_from_incomplete_or_oversized_frames() {
        for raw in [
            b"AC".as_slice(),
            b"ACL1\x01\xff\xff\xff\x7f",
            b"ACL1\x01\x0c\x00\x00\x00\x00",
            b"ACL1\x07\0\0\0\0",
        ] {
            assert!(read_frame(&mut Cursor::new(raw)).is_err());
        }
    }

    #[test]
    fn slow_pipe_preserves_the_stop_and_distinguishes_source_loss_from_overload() {
        let (tx, rx) = mpsc::sync_channel(AUDIO_SLOTS + CONTROL_RESERVE);
        let outstanding = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(OnceLock::new());
        let mut tap = Tap {
            id: "t".into(),
            sources: vec!["test".into()],
            missing: vec![None],
            tx: Some(tx),
            outstanding: outstanding.clone(),
            stop: stop.clone(),
        };
        tap.start("s", 12);
        tap.gap(0, 12, 21);
        tap.audio(0, 21, &[-1.0, -1.0, 1.0, 1.0]);
        tap.speakers(true, 23);
        let samples = [0.0; BLOCK * 2];
        let mut cursor = 23;
        // No reader drains the queue until after the cutoff.
        for _ in 0..AUDIO_SLOTS + 7 {
            tap.audio(0, cursor, &samples);
            cursor += BLOCK as u64;
        }
        tap.recording("r", 21, cursor);
        tap.finish(cursor);
        assert!(outstanding.load(Ordering::Relaxed) <= AUDIO_SLOTS);
        let mut encoded = Vec::new();
        relay(&mut encoded, rx, &outstanding, &stop).unwrap();
        let mut reader = Cursor::new(encoded);
        let mut frames = Vec::new();
        while let Some(frame) = read_frame(&mut reader).unwrap() {
            frames.push(frame);
        }
        assert!(matches!(
            frames.first(),
            Some(Frame::Start { first: 12, .. })
        ));
        assert!(frames.contains(&Frame::Gap {
            source: 0,
            first: 12,
            end: 21,
            reason: GapReason::SourceUnavailable
        }));
        assert!(frames.contains(&Frame::Audio {
            source: 0,
            first: 21,
            pcm: vec![-32768, 32767]
        }));
        assert!(frames.contains(&Frame::Speakers {
            enabled: true,
            frame: 23
        }));
        assert!(frames.contains(&Frame::Recording {
            id: "r".into(),
            start: 21,
            end: cursor
        }));
        assert!(frames.iter().any(
            |f| matches!(f,Frame::Gap {end,reason:GapReason::TapOverflow,..} if *end==cursor)
        ));
        assert_eq!(frames.last(), Some(&Frame::End { end: cursor }));
        assert_eq!(outstanding.load(Ordering::Relaxed), 0);
    }
}
