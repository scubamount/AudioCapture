//! Recording orchestration and timestamp-aligned mixing.

use crate::{capture, encode, monitor};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RATE: u64 = 48_000;
const BLOCK: u64 = 480;
const LATENCY: u64 = RATE / 5;
const MAX_AHEAD: u64 = RATE * 10;

pub struct Config {
    pub inputs: Vec<(String, capture::Input, f32)>,
    pub folder: PathBuf,
    pub format: String,
    pub rate: u32,
    pub channels: u16,
    pub bitrate: u32,
    pub flac_level: u8,
    pub separate: bool,
    pub mixed: bool,
    pub skip_silence: bool,
    pub monitor: Option<String>,
    pub monitor_only: bool,
}

struct Shared {
    stop: AtomicBool,
    done: AtomicBool,
    status: Mutex<String>,
    pauses: Mutex<Vec<(Instant, bool)>>,
}

pub struct Session {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<Result<(), String>>>,
}

impl Session {
    pub fn start(config: Config) -> Result<Self, String> {
        if config.inputs.is_empty() {
            return Err("Select at least one audio source".into());
        }
        if config.monitor_only && config.monitor.is_none() {
            return Err("Select a monitor output for monitor-only mode".into());
        }
        if !config.monitor_only && !config.separate && !config.mixed {
            return Err("Select at least one output mode".into());
        }
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            done: AtomicBool::new(false),
            status: Mutex::new("Starting capture".into()),
            pauses: Mutex::new(Vec::new()),
        });
        let state = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("recording-session".into())
            .spawn(move || {
                let result = run_session(config, &state);
                let prior = state.status.lock().unwrap().clone();
                let message = match &result {
                    Ok(()) if prior.contains("Monitor output") => {
                        format!("{prior}; recording stopped")
                    }
                    Ok(()) => "Recording stopped".to_string(),
                    Err(error) => format!("Recording stopped: {error}"),
                };
                *state.status.lock().unwrap() = message;
                state.done.store(true, Ordering::Release);
                result
            })
            .map_err(|e| format!("Could not start recording worker: {e}"))?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    pub fn pause(&self, paused: bool) {
        let mut events = self.shared.pauses.lock().unwrap();
        if events.last().map(|event| event.1).unwrap_or(false) != paused {
            events.push((Instant::now(), paused));
        }
    }

    pub fn stop(&mut self) -> Result<(), String> {
        self.shared.stop.store(true, Ordering::Release);
        match self.worker.take() {
            Some(worker) => worker
                .join()
                .map_err(|_| "Recording worker panicked".to_string())?,
            None => Ok(()),
        }
    }

    pub fn status(&self) -> String {
        self.shared.status.lock().unwrap().clone()
    }

    pub fn is_finished(&self) -> bool {
        self.shared.done.load(Ordering::Acquire)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn set_status(shared: &Shared, message: impl Into<String>) {
    *shared.status.lock().unwrap() = message.into();
}

fn safe_name(name: &str) -> String {
    let result: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = result.trim_matches([' ', '.']);
    if trimmed.is_empty() {
        "Source".into()
    } else {
        trimmed.to_string()
    }
}

fn output_path(config: &Config, name: &str, unique: u128) -> PathBuf {
    let file = format!(
        "{}-{unique}.{}",
        safe_name(name),
        config.format.to_lowercase()
    );
    config.folder.join(file)
}

fn new_writer(config: &Config, name: &str, unique: u128) -> Result<encode::Writer, String> {
    let path = output_path(config, name, unique);
    encode::Writer::open(
        &path,
        &config.format.to_lowercase(),
        config.rate,
        config.channels,
        config.bitrate,
        config.flac_level,
    )
    .map_err(|error| format!("{}: {error}", path.display()))
}

struct Source {
    name: String,
    output: Option<PathBuf>,
    gain: f32,
    rx: mpsc::Receiver<capture::Packet>,
    pending: BTreeMap<u64, Vec<f32>>,
    writer: Option<encode::Writer>,
    handle: Option<JoinHandle<Result<(), String>>>,
    ready: mpsc::Receiver<Result<(), String>>,
    started: bool,
}

// Split packets into fixed-size timeline blocks. Each source can contribute once per
// frame. Overlapping samples are replaced rather than added twice.
fn add_packet(source: &mut Source, packet: capture::Packet, cursor: u64) -> Result<(), String> {
    if packet.samples.len() % 2 != 0 {
        return Err(format!(
            "{} returned an incomplete stereo frame",
            source.name
        ));
    }
    if packet.frame < cursor && !packet.samples.is_empty() {
        return Err(format!(
            "{} delivered audio too late to keep sources synchronized",
            source.name
        ));
    }

    let frames = packet.samples.len() / 2;
    if frames > 0
        && packet.frame.saturating_add(frames as u64 - 1) > cursor.saturating_add(MAX_AHEAD)
    {
        return Err(format!(
            "{} returned an invalid future timestamp",
            source.name
        ));
    }
    let mut offset = 0;
    while offset < frames {
        let frame = packet.frame + offset as u64;
        let block = frame / BLOCK * BLOCK;
        let inside = (frame - block) as usize;
        let count = (BLOCK as usize - inside).min(frames - offset);
        let samples = source
            .pending
            .entry(block)
            .or_insert_with(|| vec![0.0; BLOCK as usize * 2]);
        let dst = &mut samples[inside * 2..(inside + count) * 2];
        let input = &packet.samples[offset * 2..(offset + count) * 2];
        if source.gain == 1.0 {
            dst.copy_from_slice(input);
        } else {
            for (to, from) in dst.iter_mut().zip(input) {
                *to = *from * source.gain;
            }
        }
        offset += count;
    }
    Ok(())
}

fn drain_packets(
    source: &mut Source,
    cursor: u64,
    max_frame: &mut u64,
    failure: &mut Option<String>,
) {
    while let Ok(packet) = source.rx.try_recv() {
        let packet_end = packet
            .frame
            .saturating_add((packet.samples.len() / 2) as u64);
        match add_packet(source, packet, cursor) {
            Ok(()) => *max_frame = (*max_frame).max(packet_end),
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
}

fn timeline_target(
    elapsed: u64,
    stop_frame: Option<u64>,
    all_started: bool,
    all_finished: bool,
    max_frame: u64,
) -> (u64, u64) {
    if stop_frame.is_some() && all_finished {
        // Include the final silent interval for the requested recording duration.
        let end = if all_started {
            stop_frame.unwrap().max(max_frame)
        } else {
            max_frame
        };
        (end.div_ceil(BLOCK) * BLOCK, end)
    } else if !all_started {
        // Do not pad an indefinitely stalled startup.
        (0, 0)
    } else {
        let wall = stop_frame.unwrap_or(elapsed);
        (wall.saturating_sub(LATENCY) / BLOCK * BLOCK, 0)
    }
}

fn render_block(
    sources: &mut [Source],
    cursor: u64,
    valid_frames: usize,
    epoch: Instant,
    pauses: &[(Instant, bool)],
    skip_silence: bool,
    mixed: &mut Option<encode::Writer>,
    monitor_tx: &mut Option<mpsc::SyncSender<Vec<f32>>>,
) -> Result<bool, String> {
    let tracks: Vec<Option<Vec<f32>>> = sources
        .iter_mut()
        .map(|source| source.pending.remove(&cursor))
        .collect();
    let audible = !skip_silence
        || tracks.iter().flatten().any(|track| {
            track[..valid_frames * 2]
                .iter()
                .any(|value| value.abs() > 0.0001)
        });
    if !audible {
        return Ok(false);
    }

    // Convert pause transitions to frame indices once per event, not per sample.
    let mut keep = vec![true; valid_frames];
    let mut paused = false;
    let mut previous = cursor;
    let end = cursor + valid_frames as u64;
    for &(at, next_paused) in pauses {
        let elapsed = at
            .checked_duration_since(epoch)
            .unwrap_or_default()
            .as_nanos();
        let event =
            ((elapsed * RATE as u128 + 999_999_999) / 1_000_000_000).min(u64::MAX as u128) as u64;
        if event <= cursor {
            paused = next_paused;
            continue;
        }
        if event >= end {
            break;
        }
        if paused {
            keep[(previous - cursor) as usize..(event - cursor) as usize].fill(false);
        }
        previous = event;
        paused = next_paused;
    }
    if paused {
        keep[(previous - cursor) as usize..].fill(false);
    }
    let kept_frames = keep.iter().filter(|keep| **keep).count();
    if kept_frames == 0 {
        return Ok(false);
    }
    let need_mix = mixed.is_some() || monitor_tx.is_some();
    let mut mix = if need_mix {
        vec![0.0_f32; kept_frames * 2]
    } else {
        Vec::new()
    };
    let silence = vec![0.0_f32; valid_frames * 2];

    for (source, track) in sources.iter_mut().zip(tracks) {
        let input = track
            .as_deref()
            .map(|samples| &samples[..valid_frames * 2])
            .unwrap_or(&silence);
        if source.writer.is_none() && !need_mix {
            continue;
        }
        let filtered;
        let selected = if kept_frames == valid_frames {
            input
        } else {
            filtered = input
                .chunks_exact(2)
                .zip(&keep)
                .filter(|(_, keep)| **keep)
                .flat_map(|(pair, _)| pair.iter().copied())
                .collect::<Vec<_>>();
            &filtered
        };
        if let Some(writer) = source.writer.as_mut() {
            writer.write(selected).map_err(|error| {
                let name = source
                    .output
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| source.name.clone());
                format!("{name}: {error}")
            })?;
        }
        if need_mix {
            for (output, sample) in mix.iter_mut().zip(selected) {
                *output += *sample;
            }
        }
    }
    for value in &mut mix {
        *value = value.clamp(-1.0, 1.0);
    }
    if let Some(writer) = mixed.as_mut() {
        writer
            .write(&mix)
            .map_err(|error| format!("Combined: {error}"))?;
    }
    if let Some(tx) = monitor_tx {
        match tx.try_send(mix) {
            Ok(()) => (),
            Err(mpsc::TrySendError::Full(_)) => {
                return Err("Monitor output could not keep up".into());
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err("Monitor output disconnected".into());
            }
        }
    }
    Ok(true)
}

fn run_session(config: Config, shared: &Shared) -> Result<(), String> {
    if !config.monitor_only {
        std::fs::create_dir_all(&config.folder)
            .map_err(|e| format!("{}: {e}", config.folder.display()))?;
    }
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let mut descriptions = Vec::new();
    for (index, (name, _, _)) in config.inputs.iter().enumerate() {
        if config.separate && !config.monitor_only {
            let path = output_path(&config, &format!("{}-{}", name, index + 1), unique);
            descriptions.push(format!("{} → {}", name, path.display()));
        } else {
            descriptions.push(name.clone());
        }
    }
    if config.mixed && !config.monitor_only {
        descriptions.push(format!(
            "Combined → {}",
            output_path(&config, "Combined", unique).display()
        ));
    }
    let description = descriptions.join(" | ");
    let mut mixed = if config.mixed && !config.monitor_only {
        Some(new_writer(&config, "Combined", unique)?)
    } else {
        None
    };
    let mut separate: Vec<Option<encode::Writer>> = Vec::new();
    for (index, (name, _, _)) in config.inputs.iter().enumerate() {
        let writer = if config.separate && !config.monitor_only {
            match new_writer(&config, &format!("{}-{}", name, index + 1), unique) {
                Ok(writer) => Some(writer),
                Err(error) => {
                    for opened in separate.into_iter().flatten() {
                        let _ = opened.finish();
                    }
                    if let Some(opened) = mixed.take() {
                        let _ = opened.finish();
                    }
                    return Err(error);
                }
            }
        } else {
            None
        };
        separate.push(writer);
    }
    let mut monitor_handle = None;
    let mut monitor_tx = if let Some(id) = config.monitor.clone() {
        let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(32);
        let (ready_tx, ready_rx) = mpsc::channel();
        monitor_handle = Some(thread::spawn(move || -> Result<(), String> {
            let mut output = match monitor::Monitor::open(&id) {
                Ok(output) => {
                    let _ = ready_tx.send(Ok(()));
                    output
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error.clone()));
                    return Err(error);
                }
            };
            while let Ok(samples) = rx.recv() {
                output.write(&samples)?;
            }
            Ok(())
        }));
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Some(tx),
            Ok(Err(error)) => {
                for opened in separate.into_iter().flatten() {
                    let _ = opened.finish();
                }
                if let Some(opened) = mixed.take() {
                    let _ = opened.finish();
                }
                return Err(format!("Monitor: {error}"));
            }
            Err(error) => {
                for opened in separate.into_iter().flatten() {
                    let _ = opened.finish();
                }
                if let Some(opened) = mixed.take() {
                    let _ = opened.finish();
                }
                return Err(format!("Monitor initialization: {error}"));
            }
        }
    } else {
        None
    };
    let epoch = Instant::now();
    let capture_stop = Arc::new(AtomicBool::new(false));
    let mut sources = Vec::new();
    for (index, ((name, input, gain), writer)) in
        config.inputs.iter().zip(separate.into_iter()).enumerate()
    {
        let (tx, rx) = mpsc::sync_channel(256);
        let (ready_tx, ready) = mpsc::channel();
        let stop = Arc::clone(&capture_stop);
        let input = input.clone();
        let handle = thread::spawn(move || capture::run(input, tx, stop, ready_tx, epoch));
        sources.push(Source {
            name: name.clone(),
            output: if writer.is_some() {
                Some(output_path(
                    &config,
                    &format!("{}-{}", name, index + 1),
                    unique,
                ))
            } else {
                None
            },
            gain: *gain,
            rx,
            pending: BTreeMap::new(),
            writer,
            handle: Some(handle),
            ready,
            started: false,
        });
    }
    let mut cursor = 0;
    let mut max_frame = 0;
    let mut failure = None;
    let mut warning = None;
    let mut last_status = Instant::now() - Duration::from_secs(1);
    let mut stopping = false;
    let mut stop_frame = None;
    let startup = Instant::now();
    loop {
        if shared.stop.load(Ordering::Acquire) || failure.is_some() {
            stopping = true;
            stop_frame.get_or_insert_with(|| (epoch.elapsed().as_secs_f64() * RATE as f64) as u64);
            capture_stop.store(true, Ordering::Release);
        }
        let mut all_finished = true;
        for source in &mut sources {
            if !source.started {
                match source.ready.try_recv() {
                    Ok(Ok(())) => source.started = true,
                    Ok(Err(error)) => {
                        failure.get_or_insert(format!("{}: {error}", source.name));
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        failure.get_or_insert(format!(
                            "{}: capture stopped during startup",
                            source.name
                        ));
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        if startup.elapsed() > Duration::from_secs(10) {
                            failure.get_or_insert(format!(
                                "{}: capture startup timed out",
                                source.name
                            ));
                        }
                    }
                };
            }
            drain_packets(source, cursor, &mut max_frame, &mut failure);
            let finished = source
                .handle
                .as_ref()
                .map(|handle| handle.is_finished())
                .unwrap_or(true);
            if finished {
                if let Some(handle) = source.handle.take() {
                    match handle.join() {
                        Ok(Ok(())) if stopping => (),
                        Ok(Ok(())) => {
                            failure.get_or_insert(format!(
                                "{}: capture ended unexpectedly",
                                source.name
                            ));
                        }
                        Ok(Err(error)) => {
                            failure.get_or_insert(format!("{}: {error}", source.name));
                        }
                        Err(_) => {
                            failure
                                .get_or_insert(format!("{}: capture worker panicked", source.name));
                        }
                    }
                    // Packets can arrive between the previous drain and observing
                    // worker completion. The sender is gone after join.
                    drain_packets(source, cursor, &mut max_frame, &mut failure);
                }
            } else {
                all_finished = false;
            }
        }
        if failure.is_some() {
            capture_stop.store(true, Ordering::Release);
            stopping = true;
            stop_frame.get_or_insert_with(|| (epoch.elapsed().as_secs_f64() * RATE as f64) as u64);
        }
        if sources.iter().all(|source| source.started)
            && !stopping
            && last_status.elapsed() >= Duration::from_secs(1)
        {
            let warning_text = warning
                .as_deref()
                .map(|w| format!(" | {w}"))
                .unwrap_or_default();
            set_status(
                shared,
                format!(
                    "Recording {}s | {}{}",
                    epoch.elapsed().as_secs(),
                    description,
                    warning_text
                ),
            );
            last_status = Instant::now();
        }
        let elapsed_frames = (epoch.elapsed().as_secs_f64() * RATE as f64) as u64;
        let (target, final_frame) = timeline_target(
            elapsed_frames,
            stop_frame,
            sources.iter().all(|source| source.started),
            all_finished,
            max_frame,
        );
        let pauses = shared.pauses.lock().unwrap().clone();
        while cursor < target {
            if let Err(error) = render_block(
                &mut sources,
                cursor,
                if stopping && all_finished {
                    (final_frame - cursor).min(BLOCK) as usize
                } else {
                    BLOCK as usize
                },
                epoch,
                &pauses,
                config.skip_silence,
                &mut mixed,
                &mut monitor_tx,
            ) {
                // Monitoring errors should not destroy an otherwise valid recording.
                if error.starts_with("Monitor output") && !config.monitor_only {
                    set_status(shared, format!("{error}; continuing recording"));
                    warning = Some(error);
                    monitor_tx.take();
                    // Audio was already written before the monitor rejected this block.
                    cursor += BLOCK;
                    continue;
                } else {
                    let error = if let Some(detail) = error.strip_prefix("Combined: ") {
                        format!(
                            "{}: {detail}",
                            output_path(&config, "Combined", unique).display()
                        )
                    } else {
                        error
                    };
                    failure.get_or_insert(error);
                    capture_stop.store(true, Ordering::Release);
                    stopping = true;
                    break;
                }
            }
            cursor += BLOCK;
        }
        if stopping && all_finished {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    drop(monitor_tx);
    if let Some(handle) = monitor_handle {
        let _ = handle.join();
    }
    for source in &mut sources {
        if let Some(writer) = source.writer.take() {
            if let Err(error) = writer.finish() {
                let name = source
                    .output
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| source.name.clone());
                failure.get_or_insert(format!("{name}: {error}"));
            }
        }
    }
    if let Some(writer) = mixed.take() {
        if let Err(error) = writer.finish() {
            failure.get_or_insert(format!(
                "{}: {error}",
                output_path(&config, "Combined", unique).display()
            ));
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(name: &str) -> Source {
        let (_, rx) = mpsc::sync_channel(1);
        let (_, ready) = mpsc::channel();
        Source {
            name: name.into(),
            output: None,
            gain: 1.0,
            rx,
            pending: BTreeMap::new(),
            writer: None,
            handle: None,
            ready,
            started: true,
        }
    }

    #[test]
    fn late_source_keeps_its_time_offset_and_mixes() {
        let mut sources = vec![source("first"), source("second")];
        add_packet(
            &mut sources[0],
            capture::Packet {
                frame: 0,
                samples: vec![0.2; BLOCK as usize * 2],
            },
            0,
        )
        .unwrap();
        add_packet(
            &mut sources[1],
            capture::Packet {
                frame: 240,
                samples: vec![0.3; 240 * 2],
            },
            0,
        )
        .unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        let mut monitor = Some(tx);
        render_block(
            &mut sources,
            0,
            BLOCK as usize,
            Instant::now(),
            &[],
            false,
            &mut None,
            &mut monitor,
        )
        .unwrap();
        let output = rx.recv().unwrap();
        assert!((output[0] - 0.2).abs() < 0.00001);
        assert!((output[239 * 2] - 0.2).abs() < 0.00001);
        assert!((output[240 * 2] - 0.5).abs() < 0.00001);
    }

    #[test]
    fn pause_and_silence_use_one_mask_for_all_sources() {
        let epoch = Instant::now();
        let mut sources = vec![source("first"), source("second")];
        add_packet(
            &mut sources[1],
            capture::Packet {
                frame: 0,
                samples: vec![0.3; BLOCK as usize * 2],
            },
            0,
        )
        .unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        let mut monitor = Some(tx);
        let pauses = vec![
            (epoch + Duration::from_millis(2), true),
            (epoch + Duration::from_millis(7), false),
        ];
        render_block(
            &mut sources,
            0,
            BLOCK as usize,
            epoch,
            &pauses,
            true,
            &mut None,
            &mut monitor,
        )
        .unwrap();
        assert_eq!(rx.recv().unwrap().len(), 240 * 2);
        assert!(
            !render_block(
                &mut sources,
                BLOCK,
                BLOCK as usize,
                epoch,
                &[],
                true,
                &mut None,
                &mut monitor
            )
            .unwrap()
        );
    }

    #[test]
    fn malformed_and_impossible_timestamps_fail() {
        let mut source = source("app");
        assert!(
            add_packet(
                &mut source,
                capture::Packet {
                    frame: 0,
                    samples: vec![0.0]
                },
                0
            )
            .is_err()
        );
        assert!(
            add_packet(
                &mut source,
                capture::Packet {
                    frame: MAX_AHEAD + 1,
                    samples: vec![0.0, 0.0]
                },
                0
            )
            .is_err()
        );
    }

    #[test]
    fn partial_final_block_keeps_only_valid_frames() {
        let mut sources = vec![source("short")];
        add_packet(
            &mut sources[0],
            capture::Packet {
                frame: 0,
                samples: vec![0.25; 97 * 2],
            },
            0,
        )
        .unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        render_block(
            &mut sources,
            0,
            97,
            Instant::now(),
            &[],
            false,
            &mut None,
            &mut Some(tx),
        )
        .unwrap();
        assert_eq!(rx.recv().unwrap().len(), 97 * 2);
    }

    #[test]
    fn clock_advances_during_a_long_silent_gap() {
        // No WASAPI packets arrived for 12 seconds. The live cursor must still
        // advance so the next valid packet stays inside the bounded queue.
        let elapsed = 12 * RATE;
        let (cursor, _) = timeline_target(elapsed, None, true, false, 0);
        assert_eq!(cursor, elapsed - LATENCY);
        let mut source = source("delayed app");
        add_packet(
            &mut source,
            capture::Packet {
                frame: elapsed,
                samples: vec![0.2; 100 * 2],
            },
            cursor,
        )
        .unwrap();
        let (target, end) = timeline_target(
            elapsed + 100,
            Some(elapsed + 100),
            true,
            true,
            elapsed + 100,
        );
        assert_eq!(end, elapsed + 100);
        assert_eq!(target, (elapsed + 100).div_ceil(BLOCK) * BLOCK);
        assert_eq!((end - elapsed).min(BLOCK), 100);
    }

    #[test]
    fn stop_keeps_silent_tail_but_startup_failure_stays_bounded() {
        let stop = RATE * 13 + 47;
        let (target, end) = timeline_target(stop, Some(stop), true, true, RATE);
        assert_eq!(end, stop);
        assert_eq!(target, stop.div_ceil(BLOCK) * BLOCK);
        let (target, end) = timeline_target(stop, Some(stop), false, true, 42);
        assert_eq!(end, 42);
        assert_eq!(target, BLOCK);
    }

    #[test]
    fn drain_after_sender_exit_keeps_final_packet() {
        let (tx, rx) = mpsc::sync_channel(2);
        let mut source = source("app");
        source.rx = rx;
        tx.send(capture::Packet {
            frame: 19,
            samples: vec![0.4; 12],
        })
        .unwrap();
        drop(tx);
        let mut end = 0;
        let mut failure = None;
        drain_packets(&mut source, 0, &mut end, &mut failure);
        assert_eq!(end, 25);
        assert!(failure.is_none());
        assert_eq!(source.pending[&0][19 * 2], 0.4);
    }

    #[test]
    fn disconnected_monitor_does_not_consume_later_blocks() {
        let mut sources = vec![source("app")];
        let (tx, rx) = mpsc::sync_channel(1);
        drop(rx);
        let mut monitor = Some(tx);
        add_packet(
            &mut sources[0],
            capture::Packet {
                frame: 0,
                samples: vec![0.25; BLOCK as usize * 2],
            },
            0,
        )
        .unwrap();
        let error = render_block(
            &mut sources,
            0,
            BLOCK as usize,
            Instant::now(),
            &[],
            false,
            &mut None,
            &mut monitor,
        )
        .unwrap_err();
        assert!(error.starts_with("Monitor output"));
        monitor.take();
        add_packet(
            &mut sources[0],
            capture::Packet {
                frame: BLOCK,
                samples: vec![0.25; BLOCK as usize * 2],
            },
            BLOCK,
        )
        .unwrap();
        assert!(
            render_block(
                &mut sources,
                BLOCK,
                BLOCK as usize,
                Instant::now(),
                &[],
                false,
                &mut None,
                &mut monitor
            )
            .unwrap()
        );
    }

    #[test]
    #[ignore]
    fn benchmark_timeline() {
        use std::hint::black_box;
        let start = Instant::now();
        let epoch = Instant::now();
        let mut sources = vec![source("one"), source("two")];
        let pauses = vec![(epoch, false)];
        for index in 0..10_000u64 {
            let frame = index * BLOCK;
            for source in &mut sources {
                add_packet(
                    source,
                    capture::Packet {
                        frame,
                        samples: vec![0.2; BLOCK as usize * 2],
                    },
                    frame,
                )
                .unwrap();
            }
            let (tx, rx) = mpsc::sync_channel(1);
            render_block(
                &mut sources,
                frame,
                BLOCK as usize,
                epoch,
                &pauses,
                false,
                &mut None,
                &mut Some(tx),
            )
            .unwrap();
            black_box(rx.recv().unwrap());
        }
        println!("timeline 10000 blocks: {:?}", start.elapsed());
    }
}
