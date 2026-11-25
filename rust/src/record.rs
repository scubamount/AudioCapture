//! Recording orchestration and timestamp-aligned mixing.

use crate::{capture, encode, markers, monitor, sources};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RATE: u64 = 48_000;
const BLOCK: u64 = 480;
const LATENCY: u64 = RATE / 5;
const MAX_AHEAD: u64 = RATE * 10;
const MAX_CAPTURE_WORKERS: usize = 64;
const STOP_GRACE: Duration = Duration::from_secs(2);
static ACTIVE_CAPTURE_WORKERS: AtomicUsize = AtomicUsize::new(0);

struct CapturePermit;

impl Drop for CapturePermit {
    fn drop(&mut self) {
        ACTIVE_CAPTURE_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

fn reserve_capture_workers(count: usize) -> Result<Vec<CapturePermit>, String> {
    let mut active = ACTIVE_CAPTURE_WORKERS.load(Ordering::Acquire);
    loop {
        let total = active
            .checked_add(count)
            .filter(|total| *total <= MAX_CAPTURE_WORKERS)
            .ok_or_else(|| "Too many capture workers are still running or stopping".to_string())?;
        match ACTIVE_CAPTURE_WORKERS.compare_exchange_weak(
            active,
            total,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => break,
            Err(actual) => active = actual,
        }
    }
    Ok((0..count).map(|_| CapturePermit).collect())
}

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
    pauses: Mutex<PauseEvents>,
    markers: Mutex<VecDeque<(Instant, String)>>,
    levels: Mutex<String>,
    monitor_only: bool,
}

#[derive(Default)]
struct PauseEvents {
    events: VecDeque<(Instant, bool)>,
    requested: bool,
}

pub struct Session {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<Result<(), String>>>,
}

impl Session {
    pub fn start(mut config: Config) -> Result<Self, String> {
        if config.inputs.is_empty() {
            return Err("Select at least one audio source".into());
        }
        if config.monitor_only && config.monitor.is_none() {
            return Err("Select a monitor output for monitor-only mode".into());
        }
        if !config.monitor_only && !config.separate && !config.mixed {
            return Err("Select at least one output mode".into());
        }
        sources::validate_routes(
            &mut config.inputs,
            (config.mixed && !config.monitor_only) || config.monitor.is_some(),
            config.monitor.as_deref(),
        )?;
        let permits = reserve_capture_workers(config.inputs.len())?;
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            done: AtomicBool::new(false),
            status: Mutex::new("Starting capture".into()),
            pauses: Mutex::new(PauseEvents::default()),
            markers: Mutex::new(VecDeque::new()),
            levels: Mutex::new(String::new()),
            monitor_only: config.monitor_only,
        });
        let state = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("recording-session".into())
            .spawn(move || {
                let result = run_session(config, &state, permits);
                let prior = state.status.lock().unwrap().clone();
                let message = match &result {
                    Ok(()) if prior.starts_with("Warning:") => {
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
        if events.requested != paused {
            events.events.push_back((Instant::now(), paused));
            events.requested = paused;
        }
    }

    /// Latest source activity text, updated by the recording worker.
    pub fn levels(&self) -> String {
        self.shared.levels.lock().unwrap().clone()
    }

    /// Queue a marker for the recorded timeline. Errors during persistence are
    /// reported in the session status.
    pub fn mark(&self, label: &str) -> Result<(), String> {
        if self.shared.monitor_only {
            return Err("Markers require a recording".into());
        }
        if self.shared.stop.load(Ordering::Acquire) || self.is_finished() {
            return Err("The recording is stopping".into());
        }
        let label = label.trim();
        if label.is_empty() || label.contains(['\n', '\r', '\t']) {
            return Err("Enter a marker label without line breaks or tabs".into());
        }
        self.shared
            .markers
            .lock()
            .unwrap()
            .push_back((Instant::now(), label.to_string()));
        Ok(())
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

fn instant_frame(epoch: Instant, at: Instant) -> u64 {
    let elapsed = at
        .checked_duration_since(epoch)
        .unwrap_or_default()
        .as_nanos();
    ((elapsed * RATE as u128 + 999_999_999) / 1_000_000_000).min(u64::MAX as u128) as u64
}

#[derive(Default)]
struct BlockSelection {
    valid: usize,
    kept: usize,
    mask: Vec<bool>,
}

impl BlockSelection {
    fn reset(&mut self, valid: usize, audible: bool) {
        self.valid = valid;
        self.kept = if audible { valid } else { 0 };
        self.mask.clear();
    }

    // Pause intervals arrive in order and do not overlap.
    fn omit(&mut self, start: usize, end: usize) {
        if start == end {
            return;
        }
        self.kept -= end - start;
        if self.kept > 0 {
            if self.mask.is_empty() {
                self.mask.resize(self.valid, true);
            }
            self.mask[start..end].fill(false);
        }
    }

    fn kept_before(&self, offset: usize) -> usize {
        if self.kept == self.valid {
            offset
        } else if self.kept == 0 {
            0
        } else {
            self.mask[..offset].iter().filter(|&&keep| keep).count()
        }
    }
}

fn commit_markers(
    requests: &mut VecDeque<(Instant, String)>,
    writer: &mut markers::MarkerWriter,
    epoch: Instant,
    cursor: u64,
    written_frames: u64,
    keep: &BlockSelection,
) -> Option<String> {
    let end = cursor + keep.valid as u64;
    let mut warning = None;
    while requests
        .front()
        .is_some_and(|(at, _)| instant_frame(epoch, *at) < end)
    {
        let (at, label) = requests.pop_front().unwrap();
        let within = instant_frame(epoch, at)
            .saturating_sub(cursor)
            .min(keep.valid as u64) as usize;
        let marker_frame = written_frames + keep.kept_before(within) as u64;
        if let Err(error) = writer.mark(marker_frame, &label) {
            warning = Some(format!("Marker: {error}"));
        }
    }
    warning
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
    let extension = if config.format.eq_ignore_ascii_case("opus-speech") {
        "opus".to_string()
    } else {
        config.format.to_lowercase()
    };
    let file = format!("{}-{unique}.{}", safe_name(name), extension);
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
    runtime_error: Option<String>,
    late_frames: u64,
    late_span: Option<(u64, u64)>,
    stats: Arc<capture::CaptureStats>,
    meter: Meter,
}

fn record_source_exit(
    source: &mut Source,
    result: Result<(), String>,
    all_started: bool,
    stopping: bool,
    failure: &mut Option<String>,
) {
    let detail = match result {
        Ok(()) if stopping => return,
        Ok(()) => "capture ended unexpectedly".to_string(),
        Err(error) => error,
    };
    let detail = format!("{}: {detail}", source.name);
    if all_started {
        source.runtime_error = Some(detail);
    } else {
        failure.get_or_insert(detail);
    }
}

#[derive(Default)]
struct Meter {
    pre: f32,
    post: f32,
    mono: f32,
    held: f32,
    hold_until: Option<Instant>,
    clipped_until: Option<Instant>,
    active_until: Option<Instant>,
}

impl Meter {
    fn observe(&mut self, input: &[f32], gain: f32) {
        let mut peak = 0.0_f32;
        let mut mono = 0.0_f32;
        for frame in input.chunks_exact(2) {
            peak = peak.max(frame[0].abs()).max(frame[1].abs());
            mono = mono.max((frame[0] + frame[1]).abs() * 0.5);
        }
        self.pre = self.pre.max(peak);
        self.post = self.post.max(peak * gain.abs());
        self.mono = self.mono.max(mono * gain.abs());
    }

    fn snapshot(&mut self, now: Instant) -> String {
        if self.pre > 0.0001 {
            self.active_until = Some(now + Duration::from_secs(1));
        }
        if self.post >= 1.0 {
            self.clipped_until = Some(now + Duration::from_secs(2));
        }
        if self.post >= self.held || self.hold_until.is_none_or(|until| now >= until) {
            self.held = self.post;
            self.hold_until = Some(now + Duration::from_secs(2));
        }
        let active = self.active_until.is_some_and(|until| now < until);
        let clipped = self.clipped_until.is_some_and(|until| now < until);
        let cancellation = self.post > 0.01 && self.mono < self.post * 0.1;
        let result = format!(
            "{} | input {:.0}% | output {:.0}% | peak {:.0}%{}{}",
            if active { "active" } else { "quiet" },
            self.pre * 100.0,
            self.post * 100.0,
            self.held * 100.0,
            if clipped { " | clipping" } else { "" },
            if cancellation {
                " | mono cancellation"
            } else {
                ""
            },
        );
        self.pre = 0.0;
        self.post = 0.0;
        self.mono = 0.0;
        result
    }
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
    let frames = packet.samples.len() / 2;
    if frames > 0
        && packet.frame.saturating_add(frames as u64 - 1) > cursor.saturating_add(MAX_AHEAD)
    {
        return Err(format!(
            "{} returned an invalid future timestamp",
            source.name
        ));
    }
    let mut offset = cursor.saturating_sub(packet.frame).min(frames as u64) as usize;
    source.late_frames = source.late_frames.saturating_add(offset as u64);
    if offset > 0 {
        let end = packet.frame.saturating_add(offset as u64);
        source.late_span = Some(match source.late_span {
            Some((start, last)) => (start.min(packet.frame), last.max(end)),
            None => (packet.frame, end),
        });
    }
    source
        .meter
        .observe(&packet.samples[offset * 2..], source.gain);
    if offset == 0 && frames == BLOCK as usize && packet.frame % BLOCK == 0 && source.gain == 1.0 {
        if let std::collections::btree_map::Entry::Vacant(entry) =
            source.pending.entry(packet.frame)
        {
            entry.insert(packet.samples);
            return Ok(());
        }
    }
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
    paused_at_cursor: bool,
    skip_silence: bool,
    mixed: &mut Option<encode::Writer>,
    monitor_tx: &mut Option<mpsc::SyncSender<Vec<f32>>>,
    keep: &mut BlockSelection,
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
    keep.reset(valid_frames, audible);
    if !audible {
        return Ok(false);
    }

    // Convert pause transitions to frame indices once per event, not per sample.
    let mut paused = paused_at_cursor;
    let mut previous = cursor;
    let end = cursor + valid_frames as u64;
    for &(at, next_paused) in pauses {
        let event = instant_frame(epoch, at);
        if event <= cursor {
            paused = next_paused;
            continue;
        }
        if event >= end {
            break;
        }
        if paused {
            keep.omit((previous - cursor) as usize, (event - cursor) as usize);
        }
        previous = event;
        paused = next_paused;
    }
    if paused {
        keep.omit((previous - cursor) as usize, valid_frames);
    }
    let kept_frames = keep.kept;
    if kept_frames == 0 {
        return Ok(false);
    }
    let need_mix = mixed.is_some() || monitor_tx.is_some();
    let mut mix = if need_mix {
        vec![0.0_f32; kept_frames * 2]
    } else {
        Vec::new()
    };
    let silence = [0.0_f32; BLOCK as usize * 2];

    for (source, track) in sources.iter_mut().zip(tracks) {
        let input = track
            .as_deref()
            .map(|samples| &samples[..valid_frames * 2])
            .unwrap_or(&silence[..valid_frames * 2]);
        if source.writer.is_none() && !need_mix {
            continue;
        }
        let filtered;
        let selected = if kept_frames == valid_frames {
            input
        } else {
            filtered = input
                .chunks_exact(2)
                .zip(keep.mask.iter())
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

fn run_session(config: Config, shared: &Shared, permits: Vec<CapturePermit>) -> Result<(), String> {
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
            loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(samples) => output.write_owned(samples)?,
                    Err(mpsc::RecvTimeoutError::Timeout) => output.check()?,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        output.check()?;
                        break;
                    }
                }
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
    for (index, (((name, input, gain), writer), permit)) in config
        .inputs
        .iter()
        .zip(separate.into_iter())
        .zip(permits)
        .enumerate()
    {
        let (tx, rx) = mpsc::sync_channel(256);
        let (ready_tx, ready) = mpsc::channel();
        let stop = Arc::clone(&capture_stop);
        let stats = Arc::new(capture::CaptureStats::default());
        let source_stats = Arc::clone(&stats);
        let input = input.clone();
        let handle = thread::spawn(move || {
            let _permit = permit;
            capture::run_with_stats(input, tx, stop, ready_tx, epoch, source_stats)
        });
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
            runtime_error: None,
            late_frames: 0,
            late_span: None,
            stats,
            meter: Meter::default(),
        });
    }
    let mut cursor = 0;
    let mut max_frame = 0;
    let mut failure = None;
    let mut warning = None;
    let mut last_status = Instant::now() - Duration::from_secs(1);
    let mut stopping = false;
    let mut stop_started = None;
    let mut stop_frame = None;
    let mut render_fatal = false;
    let mut pause_base = false;
    let mut written_frames = 0u64;
    let mut keep = BlockSelection::default();
    let mut marker_writer = if config.monitor_only {
        None
    } else {
        Some(markers::MarkerWriter::new(
            config.folder.join(format!("Markers-{unique}.labels.txt")),
            RATE as u32,
        )?)
    };
    let mut last_levels = Instant::now() - Duration::from_millis(250);
    let startup = Instant::now();
    loop {
        if shared.stop.load(Ordering::Acquire) || failure.is_some() {
            stopping = true;
            stop_started.get_or_insert_with(Instant::now);
            stop_frame.get_or_insert_with(|| (epoch.elapsed().as_secs_f64() * RATE as f64) as u64);
            capture_stop.store(true, Ordering::Release);
        }
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
        }
        let all_started = sources.iter().all(|source| source.started);
        let mut all_finished = true;
        let mut source_exited = false;
        for source in &mut sources {
            drain_packets(source, cursor, &mut max_frame, &mut failure);
            let finished = source
                .handle
                .as_ref()
                .map(|handle| handle.is_finished())
                .unwrap_or(true);
            if finished {
                if let Some(handle) = source.handle.take() {
                    let result = handle
                        .join()
                        .unwrap_or_else(|_| Err("capture worker panicked".into()));
                    record_source_exit(source, result, all_started, stopping, &mut failure);
                    source_exited |= source.runtime_error.is_some();
                    // Packets can arrive between the previous drain and observing
                    // worker completion. The sender is gone after join.
                    drain_packets(source, cursor, &mut max_frame, &mut failure);
                }
            } else {
                all_finished = false;
            }
        }
        if all_finished && !stopping && failure.is_none() {
            let errors = sources
                .iter()
                .filter_map(|source| source.runtime_error.as_deref())
                .collect::<Vec<_>>()
                .join("; ");
            failure = Some(if errors.is_empty() {
                "All recording sources ended".into()
            } else {
                format!("All recording sources ended: {errors}")
            });
        }
        if source_exited {
            last_status = Instant::now() - Duration::from_secs(1);
        }
        if monitor_handle
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            let outcome = monitor_handle.take().unwrap().join();
            monitor_tx.take();
            let detail = match outcome {
                Ok(Ok(())) => "Monitor output stopped".to_string(),
                Ok(Err(error)) => format!("Monitor output: {error}"),
                Err(_) => "Monitor output worker panicked".to_string(),
            };
            if config.monitor_only {
                failure.get_or_insert(detail);
            } else {
                warning = Some(detail.clone());
                set_status(shared, format!("Warning: {detail}; continuing recording"));
                last_status = Instant::now() - Duration::from_secs(1);
            }
        }
        if failure.is_some() {
            capture_stop.store(true, Ordering::Release);
            stopping = true;
            stop_started.get_or_insert_with(Instant::now);
            stop_frame.get_or_insert_with(|| (epoch.elapsed().as_secs_f64() * RATE as f64) as u64);
        }
        if stopping
            && !all_finished
            && stop_started.is_some_and(|at: Instant| at.elapsed() >= STOP_GRACE)
        {
            let unfinished = sources
                .iter_mut()
                .filter_map(|source| source.handle.take().map(|_| source.name.clone()))
                .collect::<Vec<_>>();
            if !unfinished.is_empty() {
                let detail = format!("Capture stop timed out: {}", unfinished.join(", "));
                failure.get_or_insert(detail);
            }
            all_finished = true;
        }
        if last_levels.elapsed() >= Duration::from_millis(250) {
            let now = Instant::now();
            let levels = sources
                .iter_mut()
                .map(|source| {
                    let known_events = source.stats.known_gap_events.load(Ordering::Relaxed);
                    let known_frames = source.stats.known_gap_frames.load(Ordering::Relaxed);
                    let uncertain = source.stats.uncertain_events.load(Ordering::Relaxed);
                    let mut detail = format!("{}: {}", source.name, source.meter.snapshot(now));
                    if source.runtime_error.is_some() {
                        detail.push_str(" | source stopped");
                    }
                    if source.late_frames > 0 {
                        let (start, end) = source.late_span.unwrap();
                        detail.push_str(&format!(
                            " | {} late frames dropped between {:.3}s and {:.3}s",
                            source.late_frames,
                            start as f64 / RATE as f64,
                            end as f64 / RATE as f64
                        ));
                    }
                    if known_events > 0 {
                        detail.push_str(&format!(
                            " | {known_events} measured gaps, {known_frames} missing frames"
                        ));
                    }
                    if uncertain > 0 {
                        detail.push_str(&format!(" | {uncertain} timing events, loss unknown"));
                    }
                    detail
                })
                .collect::<Vec<_>>()
                .join("\n");
            *shared.levels.lock().unwrap() = levels;
            last_levels = now;
        }
        if all_started && !stopping && last_status.elapsed() >= Duration::from_secs(1) {
            let mut warnings: Vec<&str> = sources
                .iter()
                .filter_map(|source| source.runtime_error.as_deref())
                .collect();
            if let Some(message) = warning.as_deref() {
                warnings.push(message);
            }
            let warning_text = if warnings.is_empty() {
                String::new()
            } else {
                format!(" | {}", warnings.join(" | "))
            };
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
        let pauses: Vec<_> = if cursor < target {
            shared
                .pauses
                .lock()
                .unwrap()
                .events
                .iter()
                .copied()
                .collect()
        } else {
            Vec::new()
        };
        while cursor < target && !render_fatal {
            let valid_frames = if stopping && all_finished {
                (final_frame - cursor).min(BLOCK) as usize
            } else {
                BLOCK as usize
            };
            let result = render_block(
                &mut sources,
                cursor,
                valid_frames,
                epoch,
                &pauses,
                pause_base,
                config.skip_silence,
                &mut mixed,
                &mut monitor_tx,
                &mut keep,
            );
            if result.is_ok()
                || result
                    .as_ref()
                    .is_err_and(|error| error.starts_with("Monitor output") && !config.monitor_only)
            {
                let end = cursor + keep.valid as u64;
                let mut requests = VecDeque::new();
                {
                    let mut pending = shared.markers.lock().unwrap();
                    while pending
                        .front()
                        .is_some_and(|(at, _)| instant_frame(epoch, *at) < end)
                    {
                        requests.push_back(pending.pop_front().unwrap());
                    }
                }
                if let Some(writer) = &mut marker_writer {
                    if let Some(error) =
                        commit_markers(&mut requests, writer, epoch, cursor, written_frames, &keep)
                    {
                        warning = Some(error);
                    }
                }
                written_frames += keep.kept as u64;
            }
            if let Err(error) = result {
                // Monitoring errors should not destroy an otherwise valid recording.
                if error.starts_with("Monitor output") && !config.monitor_only {
                    set_status(shared, format!("Warning: {error}; continuing recording"));
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
                    stop_started.get_or_insert_with(Instant::now);
                    render_fatal = true;
                    break;
                }
            }
            cursor += BLOCK;
        }
        if cursor > 0 {
            let mut pause_events = shared.pauses.lock().unwrap();
            while pause_events
                .events
                .front()
                .is_some_and(|(at, _)| instant_frame(epoch, *at) <= cursor)
            {
                pause_base = pause_events.events.pop_front().unwrap().1;
            }
        }
        if stopping && all_finished {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    drop(monitor_tx);
    if let Some(handle) = monitor_handle {
        let deadline = Instant::now() + STOP_GRACE;
        while !handle.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let monitor_failure = if handle.is_finished() {
            match handle.join() {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(format!("Monitor output: {error}")),
                Err(_) => Some("Monitor output worker panicked".to_string()),
            }
        } else {
            Some("Monitor output stop timed out".to_string())
        };
        if let Some(detail) = monitor_failure {
            if config.monitor_only {
                failure.get_or_insert(detail);
            } else {
                warning = Some(detail);
            }
        }
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
    if let Some(writer) = &mut marker_writer {
        let pending: Vec<_> = shared.markers.lock().unwrap().drain(..).collect();
        for (_, label) in pending {
            if let Err(error) = writer.mark(written_frames, &label) {
                warning = Some(format!("Marker: {error}"));
            }
        }
    }
    let mut notices = Vec::new();
    if let Some(warning) = warning {
        notices.push(warning);
    }
    for source in &sources {
        if let Some(error) = &source.runtime_error {
            notices.push(error.clone());
        }
        if source.late_frames > 0 {
            let (start, end) = source.late_span.unwrap();
            notices.push(format!(
                "{}: {} late frames dropped between {:.3}s and {:.3}s",
                source.name,
                source.late_frames,
                start as f64 / RATE as f64,
                end as f64 / RATE as f64
            ));
        }
        let known_events = source.stats.known_gap_events.load(Ordering::Relaxed);
        let known_frames = source.stats.known_gap_frames.load(Ordering::Relaxed);
        let uncertain = source.stats.uncertain_events.load(Ordering::Relaxed);
        if known_events > 0 {
            notices.push(format!(
                "{}: {known_events} measured gaps, {known_frames} missing frames",
                source.name
            ));
        }
        if uncertain > 0 {
            notices.push(format!(
                "{}: {uncertain} timing events, loss unknown",
                source.name
            ));
        }
    }
    if !notices.is_empty() {
        set_status(shared, format!("Warning: {}", notices.join("; ")));
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_opus_output_uses_standard_file_extension() {
        let mut config = Config {
            inputs: vec![],
            folder: PathBuf::from("recordings"),
            format: "opus-speech".into(),
            rate: 48_000,
            channels: 1,
            bitrate: 32,
            flac_level: 5,
            separate: true,
            mixed: false,
            skip_silence: false,
            monitor: None,
            monitor_only: false,
        };
        assert_eq!(
            output_path(&config, "app", 1),
            PathBuf::from("recordings").join("app-1.opus")
        );
        config.format = "WAV".into();
        assert_eq!(
            output_path(&config, "app", 1),
            PathBuf::from("recordings").join("app-1.wav")
        );
    }

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
            runtime_error: None,
            late_frames: 0,
            late_span: None,
            stats: Arc::new(capture::CaptureStats::default()),
            meter: Meter::default(),
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
            false,
            &mut None,
            &mut monitor,
            &mut BlockSelection::default(),
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
        let mut keep = BlockSelection::default();
        render_block(
            &mut sources,
            0,
            BLOCK as usize,
            epoch,
            &pauses,
            false,
            true,
            &mut None,
            &mut monitor,
            &mut keep,
        )
        .unwrap();
        assert_eq!(rx.recv().unwrap().len(), 240 * 2);
        assert_eq!(keep.kept, 240);
        assert_eq!(keep.kept_before(200), 96);
        assert!(
            !render_block(
                &mut sources,
                BLOCK,
                BLOCK as usize,
                epoch,
                &[],
                false,
                true,
                &mut None,
                &mut monitor,
                &mut keep,
            )
            .unwrap()
        );
        assert_eq!(keep.kept, 0);
        assert_eq!(keep.kept_before(200), 0);
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
    fn late_packet_keeps_uncommitted_suffix() {
        let mut source = source("app");
        add_packet(
            &mut source,
            capture::Packet {
                frame: BLOCK - 10,
                samples: vec![0.4; 20 * 2],
            },
            BLOCK,
        )
        .unwrap();
        assert_eq!(&source.pending[&BLOCK][..20], &[0.4; 20]);
        assert_eq!(source.late_frames, 10);
        add_packet(
            &mut source,
            capture::Packet {
                frame: 0,
                samples: vec![0.2; 20],
            },
            BLOCK,
        )
        .unwrap();
        assert_eq!(source.late_frames, 20);
    }

    #[test]
    fn aligned_packet_replacement_preserves_last_samples() {
        let mut track = source("app");
        for value in [0.2, 0.4] {
            add_packet(
                &mut track,
                capture::Packet {
                    frame: 0,
                    samples: vec![value; BLOCK as usize * 2],
                },
                0,
            )
            .unwrap();
        }
        assert!(track.pending[&0].iter().all(|sample| *sample == 0.4));
    }

    #[test]
    fn late_source_does_not_stop_healthy_source() {
        let mut tracks = vec![source("late"), source("healthy")];
        add_packet(
            &mut tracks[0],
            capture::Packet {
                frame: BLOCK - 10,
                samples: vec![0.4; 20 * 2],
            },
            BLOCK,
        )
        .unwrap();
        add_packet(
            &mut tracks[1],
            capture::Packet {
                frame: BLOCK,
                samples: vec![0.2; BLOCK as usize * 2],
            },
            BLOCK,
        )
        .unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        render_block(
            &mut tracks,
            BLOCK,
            BLOCK as usize,
            Instant::now(),
            &[],
            false,
            false,
            &mut None,
            &mut Some(tx),
            &mut BlockSelection::default(),
        )
        .unwrap();
        let mixed = rx.recv().unwrap();
        assert!((mixed[0] - 0.6).abs() < 0.00001);
        assert!((mixed[20] - 0.2).abs() < 0.00001);
        assert_eq!(tracks[0].late_span, Some((BLOCK - 10, BLOCK)));
    }

    #[test]
    fn source_failure_after_start_does_not_stop_other_source() {
        let mut failed = source("app");
        let mut failure = None;
        record_source_exit(
            &mut failed,
            Err("device disappeared".into()),
            true,
            false,
            &mut failure,
        );
        assert!(failure.is_none());
        assert_eq!(
            failed.runtime_error.as_deref(),
            Some("app: device disappeared")
        );
        let mut healthy = source("microphone");
        add_packet(
            &mut healthy,
            capture::Packet {
                frame: BLOCK,
                samples: vec![0.5; BLOCK as usize * 2],
            },
            BLOCK,
        )
        .unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        render_block(
            &mut [failed, healthy],
            BLOCK,
            BLOCK as usize,
            Instant::now(),
            &[],
            false,
            false,
            &mut None,
            &mut Some(tx),
            &mut BlockSelection::default(),
        )
        .unwrap();
        assert!(rx.recv().unwrap().iter().all(|sample| *sample == 0.5));
    }

    #[test]
    fn source_failure_during_startup_remains_fatal() {
        let mut failed = source("app");
        let mut failure = None;
        record_source_exit(
            &mut failed,
            Err("activation failed".into()),
            false,
            false,
            &mut failure,
        );
        assert_eq!(failure.as_deref(), Some("app: activation failed"));
        assert!(failed.runtime_error.is_none());
    }

    #[test]
    fn capture_worker_limit_rejects_oversized_request() {
        assert!(reserve_capture_workers(MAX_CAPTURE_WORKERS + 1).is_err());
    }

    #[test]
    fn levels_show_pre_gain_clipping_and_mono_cancellation() {
        let now = Instant::now();
        let mut meter = Meter::default();
        meter.observe(&[0.6, -0.6], 2.0);
        let first = meter.snapshot(now);
        assert!(first.contains("input 60%"));
        assert!(first.contains("output 120%"));
        assert!(first.contains("clipping"));
        assert!(first.contains("mono cancellation"));
        let held = meter.snapshot(now + Duration::from_millis(500));
        assert!(held.contains("active"));
        assert!(held.contains("peak 120%"));
        let quiet = meter.snapshot(now + Duration::from_secs(3));
        assert!(quiet.contains("quiet"));
        assert!(!quiet.contains("clipping"));
    }

    #[test]
    fn markers_follow_recorded_timeline_through_removed_samples() {
        let epoch = Instant::now();
        let path = std::env::temp_dir().join(format!(
            "capture-marker-test-{}-{}.txt",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut writer = markers::MarkerWriter::new(path.clone(), RATE as u32).unwrap();
        let mut queue = VecDeque::from([
            (epoch + Duration::from_millis(4), "during pause".to_string()),
            (epoch + Duration::from_millis(8), "after pause".to_string()),
        ]);
        let mut keep = BlockSelection::default();
        keep.reset(BLOCK as usize, true);
        keep.omit(96, 336);
        assert!(commit_markers(&mut queue, &mut writer, epoch, 0, 0, &keep).is_none());
        let labels = std::fs::read_to_string(&path).unwrap();
        assert!(labels.contains("0.002000\t0.002000\tduring pause"));
        assert!(labels.contains("0.003000\t0.003000\tafter pause"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unpaused_tail_keeps_pcm_without_allocating_a_mask() {
        let epoch = Instant::now();
        let mut track = source("tail");
        let samples = vec![0.25, -0.75, 0.5, -0.5];
        add_packet(
            &mut track,
            capture::Packet {
                frame: BLOCK,
                samples: samples.clone(),
            },
            BLOCK,
        )
        .unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        let mut keep = BlockSelection::default();
        // Resume exactly at the cursor. The next pause belongs to a later block.
        let pauses = [
            (epoch + Duration::from_millis(10), false),
            (epoch + Duration::from_millis(20), true),
        ];
        render_block(
            &mut [track],
            BLOCK,
            2,
            epoch,
            &pauses,
            true,
            false,
            &mut None,
            &mut Some(tx),
            &mut keep,
        )
        .unwrap();
        assert_eq!(rx.recv().unwrap(), samples);
        assert_eq!(keep.kept, 2);
        assert_eq!(keep.kept_before(1), 1);
        assert_eq!(keep.mask.capacity(), 0);
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
            false,
            &mut None,
            &mut Some(tx),
            &mut BlockSelection::default(),
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
            false,
            &mut None,
            &mut monitor,
            &mut BlockSelection::default(),
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
                false,
                &mut None,
                &mut monitor,
                &mut BlockSelection::default(),
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
                false,
                &mut None,
                &mut Some(tx),
                &mut BlockSelection::default(),
            )
            .unwrap();
            black_box(rx.recv().unwrap());
        }
        println!("timeline 10000 blocks: {:?}", start.elapsed());
    }
}
