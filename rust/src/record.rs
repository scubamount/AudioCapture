//! Recording orchestration and timestamp-aligned mixing.

mod disk;
mod intervals;
mod live;
use disk::{MarkerSink, Recording};
pub use live::LiveTap;
use live::{Activities, Command, Consumers, next_id};

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

#[derive(Clone)]
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
    id: String,
    activities: Mutex<Activities>,
    controls_pending: AtomicBool,
    recording_route_error: Option<String>,
    activated: AtomicBool,
    receipts: live::RecordingReceipts,
    stop: AtomicBool,
    cutoff: Mutex<Option<Instant>>,
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
    pub fn start(config: Config) -> Result<Self, String> {
        Self::acquire(config, None)
    }

    pub fn start_live(config: Config, tap: Box<dyn LiveTap>) -> Result<Self, String> {
        Self::acquire(config, Some(tap))
    }

    fn acquire(mut config: Config, initial_tap: Option<Box<dyn LiveTap>>) -> Result<Self, String> {
        let initial_recording = initial_tap.is_none() && !config.monitor_only;
        if initial_tap.is_some() && config.monitor_only {
            return Err("Turn off Monitor only before starting transcription".into());
        }
        if config.inputs.is_empty() {
            return Err("Select at least one audio source".into());
        }
        if config.monitor_only && config.monitor.is_none() {
            return Err("Select a monitor output for monitor-only mode".into());
        }
        if initial_recording && !config.separate && !config.mixed {
            return Err("Select at least one output mode".into());
        }
        sources::validate_routes(
            &mut config.inputs,
            initial_tap.is_none()
                && ((config.mixed && !config.monitor_only) || config.monitor.is_some()),
            if initial_tap.is_none() {
                config.monitor.as_deref()
            } else {
                None
            },
        )?;
        // Check optional writer routes once while pinning the sources. Starting a
        // writer later must not enumerate devices or processes on the mixer.
        let recording_route_error = if initial_tap.is_some()
            && (config.mixed || config.monitor.is_some())
        {
            sources::validate_routes(&mut config.inputs.clone(), true, config.monitor.as_deref())
                .err()
        } else {
            None
        };
        let permits = reserve_capture_workers(config.inputs.len())?;
        let shared = Arc::new(Shared {
            id: format!("session-{}-{}", std::process::id(), next_id()),
            activities: Mutex::new(Activities::new(initial_recording, initial_tap.is_some())),
            controls_pending: AtomicBool::new(false),
            recording_route_error,
            activated: AtomicBool::new(false),
            receipts: Default::default(),
            stop: AtomicBool::new(false),
            cutoff: Mutex::new(None),
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
                let result = run_session(config, &state, permits, initial_tap);
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
                let mut ownership = state.activities.lock().unwrap();
                ownership.recording = false;
                ownership.live = false;
                ownership.events.clear();
                result
            })
            .map_err(|e| format!("Could not start recording worker: {e}"))?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    pub fn session_id(&self) -> &str {
        &self.shared.id
    }
    /// Return the next finalized recording or its error. Polling consumes one receipt.
    pub fn take_recording_result(&self) -> Option<Result<PathBuf, String>> {
        self.shared.receipts.lock().unwrap().pop_front()
    }
    pub fn has_activated(&self) -> bool {
        self.shared.activated.load(Ordering::Acquire)
    }
    pub fn recording(&self) -> bool {
        self.shared.activities.lock().unwrap().recording
    }
    pub fn transcribing(&self) -> bool {
        self.shared.activities.lock().unwrap().live
    }

    fn submit(&self, command: Command) -> Result<(), String> {
        let mut activities = self.shared.activities.lock().unwrap();
        if self.is_finished() || self.shared.stop.load(Ordering::Acquire) {
            return Err("Capture session is stopping".into());
        }
        if activities.events.len() >= 64 && !matches!(command, Command::StopBoth) {
            return Err("Too many pending capture controls".into());
        }
        if self.shared.monitor_only && !matches!(command, Command::StopBoth) {
            return Err("Stop monitoring before changing capture activities".into());
        }
        if activities.change(&command)? {
            let at = Instant::now();
            if matches!(command, Command::StartRecording) {
                let mut pauses = self.shared.pauses.lock().unwrap();
                if pauses.requested {
                    pauses.requested = false;
                    pauses.events.push_back((at, false));
                }
            }
            activities.events.push_back((at, command));
            self.shared.controls_pending.store(true, Ordering::Release);
            if !activities.recording && !activities.live {
                *self.shared.cutoff.lock().unwrap() = Some(at);
                self.shared.stop.store(true, Ordering::Release);
            }
        }
        Ok(())
    }

    pub fn attach_live(&self, tap: Box<dyn LiveTap>) -> Result<(), String> {
        self.submit(Command::Attach(tap))
    }
    pub fn start_recording(&self) -> Result<(), String> {
        self.submit(Command::StartRecording)
    }
    pub fn stop_recording(&self) -> Result<(), String> {
        self.submit(Command::StopRecording)
    }
    pub fn stop_live(&self) -> Result<(), String> {
        self.submit(Command::StopLive)
    }
    pub fn stop_both(&self) -> Result<(), String> {
        self.submit(Command::StopBoth)
    }
    pub fn set_live_speakers(&self, enabled: bool) -> Result<(), String> {
        self.submit(Command::Speakers(enabled))
    }

    pub fn pause(&self, paused: bool) {
        if !self.recording() && !self.shared.monitor_only {
            return;
        }
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
        if !self.recording() {
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
        let _ = self.stop_both();
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
    audible: bool,
    valid: usize,
    kept: usize,
    mask: Vec<bool>,
}

impl BlockSelection {
    fn reset(&mut self, valid: usize, audible: bool) {
        self.audible = audible;
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
    writer: &mut impl MarkerSink,
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
    .map_err(|error| format!("{error}: {}", path.display()))
}

struct Source {
    name: String,
    gain: f32,
    rx: mpsc::Receiver<(capture::Packet, capture::TimingEvent)>,
    timing: live::TimingNotices,
    pending: BTreeMap<u64, Vec<f32>>,
    handle: Option<JoinHandle<Result<(), String>>>,
    ready: mpsc::Receiver<Result<(), String>>,
    started: bool,
    runtime_error: Option<String>,
    unavailable_from: Option<u64>,
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
        source
            .timing
            .observe(cursor, capture::TimingEvent::Uncertain);
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
    while let Ok((packet, timing)) = source.rx.try_recv() {
        source.timing.observe(packet.frame, timing);
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
    recording: &mut Option<Recording>,
    monitor_tx: &mut Option<mpsc::SyncSender<Vec<f32>>>,
    keep: &mut BlockSelection,
) -> Result<bool, String> {
    let block = cursor / BLOCK * BLOCK;
    let offset = (cursor - block) as usize * 2;
    let samples = offset..offset + valid_frames * 2;
    let audible = !skip_silence
        || sources.iter().any(|source| {
            source.pending.get(&block).is_some_and(|track| {
                track[samples.clone()]
                    .iter()
                    .any(|value| value.abs() > 0.0001)
            })
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
    let need_mix = recording.as_ref().is_some_and(|r| r.mixed.is_some()) || monitor_tx.is_some();
    let mut mix = if need_mix {
        vec![0.0_f32; kept_frames * 2]
    } else {
        Vec::new()
    };
    let silence = [0.0_f32; BLOCK as usize * 2];

    for (index, source) in sources.iter().enumerate() {
        let input = source
            .pending
            .get(&block)
            .map(|t| &t[samples.clone()])
            .unwrap_or(&silence[..valid_frames * 2]);
        let writer = recording.as_mut().and_then(|r| r.files[index].as_mut());
        if writer.is_none() && !need_mix {
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
        if let Some(writer) = writer {
            writer.write(selected)?;
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
    if let Some(writer) = recording.as_mut().and_then(|r| r.mixed.as_mut()) {
        writer.write(&mix)?;
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

fn spawn_monitor(
    id: String,
) -> (
    mpsc::SyncSender<Vec<f32>>,
    JoinHandle<Result<(), String>>,
    mpsc::Receiver<Result<(), String>>,
) {
    let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(32);
    let (ready_tx, ready_rx) = mpsc::channel();
    let handle = thread::spawn(move || -> Result<(), String> {
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
    });
    (tx, handle, ready_rx)
}

fn run_session(
    config: Config,
    shared: &Shared,
    permits: Vec<CapturePermit>,
    initial_tap: Option<Box<dyn LiveTap>>,
) -> Result<(), String> {
    let recording = if initial_tap.is_none() && !config.monitor_only {
        Some(Recording::open(&config, 0)?)
    } else {
        None
    };
    let mut consumers = Consumers {
        closing: Vec::new(),
        receipts: shared.receipts.clone(),
        recording,
        live: None,
    };
    if let Some(tap) = initial_tap {
        consumers.attach(tap, &config, &shared.id, 0)?;
    }
    let description = config
        .inputs
        .iter()
        .map(|(name, _, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    let mut monitor_handle = None;
    let mut monitor_tx =
        if let Some(id) = config.monitor.clone().filter(|_| consumers.live.is_none()) {
            let (tx, handle, ready_rx) = spawn_monitor(id);
            monitor_handle = Some(handle);
            match ready_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(())) => Some(tx),
                Ok(Err(error)) => return Err(format!("Monitor: {error}")),
                Err(error) => return Err(format!("Monitor initialization: {error}")),
            }
        } else {
            None
        };
    let epoch = Instant::now();
    let capture_stop = Arc::new(AtomicBool::new(false));
    let mut sources = Vec::new();
    for ((name, input, gain), permit) in config.inputs.iter().zip(permits) {
        let (tx, rx) = mpsc::sync_channel(256);
        let (ready_tx, ready) = mpsc::channel();
        let stop = Arc::clone(&capture_stop);
        let stats = Arc::new(capture::CaptureStats::default());
        let source_stats = Arc::clone(&stats);
        let input = input.clone();
        let handle = thread::spawn(move || {
            let _permit = permit;
            capture::run_timed(input, tx, stop, ready_tx, epoch, source_stats)
        });
        sources.push(Source {
            name: name.clone(),
            gain: *gain,
            rx,
            timing: live::TimingNotices::default(),
            pending: BTreeMap::new(),
            handle: Some(handle),
            ready,
            started: false,
            runtime_error: None,
            unavailable_from: None,
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
    let mut keep = BlockSelection::default();
    let mut controls = VecDeque::new();
    let mut last_levels = Instant::now() - Duration::from_millis(250);
    let startup = Instant::now();
    loop {
        if shared.controls_pending.swap(false, Ordering::AcqRel) {
            controls.append(&mut shared.activities.lock().unwrap().events);
        }
        if shared.stop.load(Ordering::Acquire) || failure.is_some() {
            stopping = true;
            stop_started.get_or_insert_with(Instant::now);
            stop_frame.get_or_insert_with(|| {
                let at = shared.cutoff.lock().unwrap().unwrap_or_else(Instant::now);
                instant_frame(epoch, at)
            });
            // A stop may arrive between the first command poll and the stop
            // flag. Observe its precise cutoff before draining the last block.
            if shared.controls_pending.swap(false, Ordering::AcqRel) {
                controls.append(&mut shared.activities.lock().unwrap().events);
            }
            capture_stop.store(true, Ordering::Release);
        }
        for source in &mut sources {
            if !source.started {
                match source.ready.try_recv() {
                    Ok(Ok(())) => source.started = true,
                    Ok(Err(error)) => {
                        source.unavailable_from = Some(0);
                        failure.get_or_insert(format!("{}: {error}", source.name));
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        source.unavailable_from = Some(0);
                        failure.get_or_insert(format!(
                            "{}: capture stopped during startup",
                            source.name
                        ));
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        if startup.elapsed() > Duration::from_secs(10) {
                            source.unavailable_from = Some(0);
                            failure.get_or_insert(format!(
                                "{}: capture startup timed out",
                                source.name
                            ));
                        }
                    }
                };
            }
        }
        if consumers.recording.as_ref().is_some_and(Recording::closed) {
            let _ = consumers.stop_recording(&config, cursor, false);
            let mut ownership = shared.activities.lock().unwrap();
            ownership.recording = false;
            if !ownership.live && !config.monitor_only {
                shared.stop.store(true, Ordering::Release);
            }
        }
        if let Err(error) = consumers.poll_finalizers() {
            set_status(shared, format!("Recording writer: {error}"));
            if consumers.live.is_some() || consumers.recording.is_some() {
                warning = Some(error);
            } else {
                failure.get_or_insert(error);
            }
        }
        let all_started = sources.iter().all(|source| source.started);
        if !shared.activated.load(Ordering::Relaxed)
            && all_started
            && failure.is_none()
            && (config.monitor_only
                || consumers.live.is_some()
                || consumers.recording.as_ref().is_some_and(Recording::ready))
        {
            shared.activated.store(true, Ordering::Release);
        }
        let track_timing = consumers.live.is_some()
            || consumers
                .recording
                .as_ref()
                .is_some_and(|r| r.map.is_some())
            || controls
                .iter()
                .any(|(_, e)| matches!(e, Command::Attach(_)));
        let mut all_finished = true;
        let mut source_exited = false;
        for source in &mut sources {
            source.timing.enable(track_timing);
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
                    if source.runtime_error.is_some() {
                        let observed = instant_frame(epoch, Instant::now());
                        source.unavailable_from = Some(observed);
                        source
                            .timing
                            .observe(observed, capture::TimingEvent::Uncertain);
                        source_exited = true;
                    }
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
                    "{} {}s | {}{}",
                    if consumers.recording.is_some() {
                        "Recording"
                    } else if consumers.live.is_some() {
                        "Live transcription"
                    } else {
                        "Monitoring"
                    },
                    epoch.elapsed().as_secs(),
                    consumers
                        .recording
                        .as_ref()
                        .map(|r| r.describe())
                        .unwrap_or_else(|| description.clone()),
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
        let render_end = if stopping && all_finished {
            final_frame
        } else {
            target
        };
        while cursor <= render_end && !render_fatal {
            while controls
                .front()
                .is_some_and(|(at, _)| instant_frame(epoch, *at) <= cursor)
            {
                let (_, command) = controls.pop_front().unwrap();
                let started_record = matches!(command, Command::StartRecording);
                if let Err(error) = consumers.apply(command, &config, shared, cursor) {
                    warning = Some(error.clone());
                    set_status(shared, format!("Warning: {error}"));
                }
                if started_record
                    && consumers.recording.is_some()
                    && let Some(id) = config.monitor.as_ref()
                    && monitor_handle.is_none()
                {
                    let (tx, handle, _ready) = spawn_monitor(id.clone());
                    monitor_handle = Some(handle);
                    monitor_tx = Some(tx);
                }
            }
            if cursor == render_end {
                break;
            }
            let block = cursor / BLOCK * BLOCK;
            let block_end = (block + BLOCK).min(render_end);
            let end = controls
                .front()
                .map(|(at, _)| instant_frame(epoch, *at))
                .unwrap_or(block_end)
                .min(block_end);
            let valid = (end - cursor) as usize;
            if let Err(error) = consumers.send_audio(&mut sources, cursor, valid) {
                if let Some(r) = &mut consumers.recording {
                    r.incomplete = true;
                }
                warning = Some(format!("Recording timeline: {error}"));
            }
            if stopping
                && all_finished
                && consumers.live.is_none()
                && !controls
                    .iter()
                    .any(|(_, cmd)| matches!(cmd, Command::Attach(_)))
                && let Some(r) = &consumers.recording
            {
                r.allow_final_drain();
            }
            let result = render_block(
                &mut sources,
                cursor,
                valid,
                epoch,
                &pauses,
                pause_base,
                config.skip_silence && (consumers.recording.is_some() || config.monitor_only),
                &mut consumers.recording,
                &mut monitor_tx,
                &mut keep,
            );
            let monitor_failed = result
                .as_ref()
                .is_err_and(|e| e.starts_with("Monitor output"))
                && !config.monitor_only;
            if (result.is_ok() || monitor_failed)
                && let Some(r) = &mut consumers.recording
            {
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
                if let Some(error) = commit_markers(
                    &mut requests,
                    &mut r.markers,
                    epoch,
                    cursor,
                    r.written,
                    &keep,
                ) {
                    warning = Some(error);
                }
                if let Some(map) = &mut r.map
                    && let Err(error) = map.block(cursor, r.written, &keep)
                {
                    warning = Some(format!("Recording timeline: {error}"));
                    r.incomplete = true;
                    r.map = None;
                }
                r.written += keep.kept as u64;
            }
            if let Err(error) = result {
                if monitor_failed {
                    warning = Some(error);
                    monitor_tx.take();
                } else if consumers.recording.is_some() && consumers.live.is_some() {
                    let _ = consumers.stop_recording(&config, cursor, false);
                    shared.activities.lock().unwrap().recording = false;
                    set_status(shared, format!("Warning: {error}"));
                    warning = Some(error);
                } else {
                    failure.get_or_insert(error);
                    render_fatal = true;
                }
            }
            cursor = end;
            if cursor == block + BLOCK || cursor == final_frame && stopping && all_finished {
                for source in &mut sources {
                    source.pending.remove(&block);
                }
            }
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
    if let Some(r) = consumers.recording.as_mut() {
        let pending: Vec<_> = shared.markers.lock().unwrap().drain(..).collect();
        for (_, label) in pending {
            if let Err(e) = r.markers.mark(r.written, &label) {
                warning = Some(format!("Marker: {e}"));
            }
        }
    }
    if let Err(error) = consumers.stop_live(cursor) {
        failure.get_or_insert(error);
    }
    let final_output = consumers.stop_recording(&config, cursor, failure.is_none());
    if let Err(error) = final_output {
        failure.get_or_insert(error);
    }
    if let Err(e) = consumers.wait_finalizers() {
        failure.get_or_insert(e);
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
            gain: 1.0,
            rx,
            timing: live::TimingNotices::default(),
            pending: BTreeMap::new(),
            handle: None,
            ready,
            started: true,
            runtime_error: None,
            unavailable_from: None,
            late_frames: 0,
            late_span: None,
            stats: Arc::new(capture::CaptureStats::default()),
            meter: Meter::default(),
        }
    }

    #[test]
    fn live_control_requests_keep_the_other_consumer() {
        let mut control = Activities::new(true, false);
        control.change(&Command::Attach(Box::new(TestTap))).unwrap();
        assert!(control.recording && control.live);
        control.change(&Command::StopRecording).unwrap();
        assert!(!control.recording && control.live);
        control.change(&Command::StartRecording).unwrap();
        control.change(&Command::StopLive).unwrap();
        assert!(control.recording && !control.live);
        control.change(&Command::StopBoth).unwrap();
        assert!(!control.live && !control.recording);
    }

    struct TestTap;
    impl LiveTap for TestTap {
        fn interval_id(&self) -> &str {
            "test-live"
        }
        fn start(&mut self, _: &str, _: u64) {}
        fn audio(&mut self, _: usize, _: u64, _: &[f32]) {}
        fn gap(&mut self, _: usize, _: u64, _: u64) {}
        fn finish(&mut self, _: u64) {}
    }

    #[derive(Default)]
    struct Trace {
        audio: Vec<(usize, u64, usize, f32)>,
        cutoffs: Vec<u64>,
        overlaps: Vec<(String, u64, u64)>,
        gaps: Vec<(usize, u64, u64)>,
        uncertain: Vec<(usize, u64)>,
    }
    struct TraceTap(Arc<Mutex<Trace>>);
    impl LiveTap for TraceTap {
        fn interval_id(&self) -> &str {
            "live-001"
        }
        fn start(&mut self, _: &str, first: u64) {
            self.0.lock().unwrap().cutoffs.push(first);
        }
        fn audio(&mut self, source: usize, first: u64, stereo: &[f32]) {
            self.0
                .lock()
                .unwrap()
                .audio
                .push((source, first, stereo.len() / 2, stereo[0]));
        }
        fn gap(&mut self, source: usize, first: u64, end: u64) {
            self.0.lock().unwrap().gaps.push((source, first, end));
        }
        fn recording(&mut self, id: &str, start: u64, end: u64) {
            self.0
                .lock()
                .unwrap()
                .overlaps
                .push((id.to_string(), start, end));
        }
        fn uncertain(&mut self, source: usize, at: u64) {
            self.0.lock().unwrap().uncertain.push((source, at));
        }
        fn finish(&mut self, end: u64) {
            self.0.lock().unwrap().cutoffs.push(end);
        }
    }

    fn live_recording_config() -> Config {
        Config {
            inputs: vec![
                ("app".into(), capture::Input::Process(0), 1.0),
                ("mic".into(), capture::Input::Mic("test".into()), 1.0),
            ],
            folder: std::env::temp_dir().join(format!(
                "capture-interval-test-{}-{}",
                std::process::id(),
                next_id()
            )),
            format: "wav".into(),
            rate: 48000,
            channels: 1,
            bitrate: 32,
            flac_level: 5,
            separate: true,
            mixed: false,
            skip_silence: true,
            monitor: None,
            monitor_only: false,
        }
    }

    #[test]
    fn staggered_consumers_preserve_pcm_bounds_map_and_markers() {
        let config = live_recording_config();
        let mut sources = vec![source("app"), source("mic")];
        add_packet(
            &mut sources[0],
            capture::Packet {
                frame: 0,
                samples: vec![0.25; 960],
            },
            0,
        )
        .unwrap();
        let trace = Arc::new(Mutex::new(Trace::default()));
        let mut active = Consumers {
            closing: Vec::new(),
            receipts: Default::default(),
            recording: None,
            live: None,
        };
        active
            .attach(Box::new(TraceTap(trace.clone())), &config, "session-001", 0)
            .unwrap();
        active.send_audio(&mut sources, 0, 96).unwrap();
        assert!(
            !config.folder.exists(),
            "text-only capture must not create the audio folder"
        );
        let mut recording = Recording::open(&config, 96).unwrap();
        recording.enable_map(&config, "session-001", 96).unwrap();
        let id = recording.id.clone();
        active.recording = Some(recording);
        let epoch = Instant::now();
        let pauses = [
            (epoch + Duration::from_millis(4), true),
            (epoch + Duration::from_millis(6), false),
        ];
        let mut keep = BlockSelection::default();
        let mut markers =
            VecDeque::from([(epoch + Duration::from_millis(5), "inside pause".to_string())]);
        active.send_audio(&mut sources, 96, 336).unwrap();
        render_block(
            &mut sources,
            96,
            336,
            epoch,
            &pauses,
            false,
            true,
            &mut active.recording,
            &mut None,
            &mut keep,
        )
        .unwrap();
        let r = active.recording.as_mut().unwrap();
        assert_eq!(keep.kept, 240);
        r.map.as_mut().unwrap().block(96, r.written, &keep).unwrap();
        commit_markers(&mut markers, &mut r.markers, epoch, 96, r.written, &keep);
        r.written += keep.kept as u64;
        active.stop_recording(&config, 432, true).unwrap();
        active.send_audio(&mut sources, 432, 48).unwrap();
        sources[1].runtime_error = Some("mic disconnected".into());
        sources[1].unavailable_from = Some(480);
        active.send_audio(&mut sources, 480, 48).unwrap();
        active.stop_live(528).unwrap();
        active.wait_finalizers().unwrap();
        let trace = trace.lock().unwrap();
        assert_eq!(trace.cutoffs, [0, 528]);
        assert_eq!(trace.overlaps, [(id.clone(), 96, 432)]);
        assert_eq!(trace.gaps, [(1, 480, 528)]);
        assert_eq!(trace.audio[2], (0, 96, 336, 0.25));
        let wav = std::fs::read(config.folder.join(format!("app-1-{id}.wav"))).unwrap();
        assert_eq!(wav.len(), 44 + 240 * 2);
        let pcm = i16::from_le_bytes([wav[44], wav[45]]);
        assert!(pcm == 8191 || pcm == 8192);
        assert!(
            wav[44..]
                .chunks_exact(2)
                .all(|p| i16::from_le_bytes([p[0], p[1]]) == pcm)
        );
        let path = config.folder.join(format!("Recording-{id}.timeline.jsonl"));
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(lines[0]["outputs"][0]["file"], format!("app-1-{id}.wav"));
        assert_eq!(
            lines.iter().find(|r| r["kind"] == "pause").unwrap()["start"],
            192
        );
        assert_eq!(
            lines.iter().find(|r| r["type"] == "transcript").unwrap()["end"],
            432
        );
        let identity = lines.iter().find(|r| r["type"] == "file_identity").unwrap();
        assert_eq!(identity["bytes"], 44 + 240 * 2);
        assert!(identity["last_write"].as_u64().unwrap() > 0);
        assert!(identity["file_index"].as_u64().unwrap() > 0);
        assert_eq!(lines.last().unwrap()["saved_frames"], 240);
        assert_eq!(lines.last().unwrap()["complete"], true);
        let mark = std::fs::read_to_string(config.folder.join(format!("Markers-{id}.labels.txt")))
            .unwrap();
        assert!(
            mark.starts_with("0.002000\t0.002000\tinside pause"),
            "{mark}"
        );
        std::fs::remove_dir_all(config.folder).unwrap();
    }

    #[test]
    fn stop_burst_can_queue_the_entire_alignment_window_before_disk_resumes() {
        let config = live_recording_config();
        let mut r = Recording::open(&config, 0).unwrap();
        let wav = r.first_path.clone();
        let (entered, release) = r.stall();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let audio = [0.25f32; BLOCK as usize * 2];
        for _ in 0..LATENCY / BLOCK {
            for output in r.files.iter_mut().flatten() {
                output.write(&audio).unwrap();
            }
            r.written += BLOCK;
        }
        release.send(()).unwrap();
        r.finalize(LATENCY, true).join().unwrap().unwrap();
        assert_eq!(std::fs::metadata(wav).unwrap().len(), 44 + 2 * LATENCY);
        std::fs::remove_dir_all(config.folder).unwrap();
    }

    #[test]
    fn stalled_disk_preserves_live_cutoff_and_atomic_stem_lengths() {
        let config = live_recording_config();
        let mut r = Recording::open(&config, 0).unwrap();
        let id = r.id.clone();
        let trace = Arc::new(Mutex::new(Trace::default()));
        let mut active = Consumers {
            closing: Vec::new(),
            receipts: Default::default(),
            recording: None,
            live: None,
        };
        r.enable_map(&config, "stall", 0).unwrap();
        let (entered, release) = r.stall();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let audio = [0.25f32; BLOCK as usize * 2];
        let mut count = 0;
        while r.files[0].as_mut().unwrap().write(&audio).is_ok() {
            if r.files[1].as_mut().unwrap().write(&audio).is_err() {
                break;
            }
            count += 1;
        }
        assert!(count > 0 && count <= 256);
        r.written = count * BLOCK;
        active.recording = Some(r);
        active
            .attach(Box::new(TraceTap(trace.clone())), &config, "stall", 0)
            .unwrap();
        let frame = count * BLOCK;
        active.stop_recording(&config, frame, false).unwrap();
        let mut sources = vec![source("app"), source("mic")];
        active.send_audio(&mut sources, frame, 25).unwrap();
        active.stop_live(frame + 25).unwrap();
        assert_eq!(trace.lock().unwrap().cutoffs, [0, frame + 25]);
        assert!(!active.closing[0].1.is_finished());
        release.send(()).unwrap();
        assert!(active.wait_finalizers().is_err());
        for stem in ["app-1", "mic-2"] {
            assert_eq!(
                std::fs::metadata(config.folder.join(format!("{stem}-{id}.wav")))
                    .unwrap()
                    .len(),
                44 + count * BLOCK * 2
            );
        }
        std::fs::remove_dir_all(config.folder).unwrap();
    }

    #[test]
    fn failed_disk_preserves_accepted_duration_and_live_gap_quality() {
        let config = live_recording_config();
        let mut r = Recording::open(&config, 0).unwrap();
        let id = r.id.clone();
        let mut sources = vec![source("app"), source("mic")];
        let trace = Arc::new(Mutex::new(Trace::default()));
        let audio = [0.25; BLOCK as usize * 2];
        r.enable_map(&config, "quality", 0).unwrap();
        for f in r.files.iter_mut().flatten() {
            f.write(&audio).unwrap();
        }
        let mut keep = BlockSelection::default();
        keep.reset(BLOCK as usize, true);
        r.map.as_mut().unwrap().block(0, 0, &keep).unwrap();
        r.written = BLOCK;
        let mut active = Consumers {
            closing: Vec::new(),
            receipts: Default::default(),
            recording: Some(r),
            live: None,
        };
        active
            .attach(Box::new(TraceTap(trace.clone())), &config, "quality", 0)
            .unwrap();
        sources[0].timing.enable(true);
        sources[0]
            .timing
            .observe(192, capture::TimingEvent::KnownGap(96));
        sources[0]
            .timing
            .observe(260, capture::TimingEvent::Uncertain);
        active.send_audio(&mut sources, 0, BLOCK as usize).unwrap();
        assert_eq!(trace.lock().unwrap().gaps, [(0, 96, 192)]);
        assert_eq!(trace.lock().unwrap().uncertain, [(0, 260)]);
        assert_eq!(trace.lock().unwrap().audio[0], (0, 0, 96, 0.0));
        assert_eq!(trace.lock().unwrap().audio[1], (0, 192, 288, 0.0));
        let r = active.recording.as_ref().unwrap();
        r.inject_failure();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !r.closed() && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(r.closed());
        active.stop_recording(&config, BLOCK, false).unwrap();
        active.stop_live(BLOCK).unwrap();
        assert!(active.wait_finalizers().unwrap_err().contains("Injected"));
        let map =
            std::fs::read_to_string(config.folder.join(format!("Recording-{id}.timeline.jsonl")))
                .unwrap();
        let tail: serde_json::Value = serde_json::from_str(map.lines().last().unwrap()).unwrap();
        assert_eq!(tail["end"], BLOCK);
        assert_eq!(tail["saved_frames"], BLOCK);
        assert_eq!(tail["complete"], false);
        assert!(map.contains("timing_uncertain"));
        assert!(map.contains("source_gap"));
        std::fs::remove_dir_all(config.folder).unwrap();
    }

    #[test]
    fn stop_live_first_leaves_writer_and_distinct_restarts() {
        let config = live_recording_config();
        let trace = Arc::new(Mutex::new(Trace::default()));
        let first = Recording::open(&config, 0).unwrap();
        let id = first.id.clone();
        let mut active = Consumers {
            closing: Vec::new(),
            receipts: Default::default(),
            recording: Some(first),
            live: None,
        };
        active
            .attach(
                Box::new(TraceTap(trace.clone())),
                &config,
                "session-002",
                53,
            )
            .unwrap();
        active.stop_live(87).unwrap();
        assert!(active.recording.is_some());
        active
            .attach(
                Box::new(TraceTap(trace.clone())),
                &config,
                "session-002",
                100,
            )
            .unwrap();
        active.stop_live(120).unwrap();
        active.stop_recording(&config, 120, true).unwrap();
        assert_eq!(trace.lock().unwrap().cutoffs, [53, 87, 100, 120]);
        assert_eq!(
            trace.lock().unwrap().overlaps,
            [(id.clone(), 53, 87), (id.clone(), 100, 120)]
        );
        let second = Recording::open(&config, 120).unwrap();
        assert_ne!(id, second.id);
        second.finalize(120, true).join().unwrap().unwrap();
        active.wait_finalizers().unwrap();
        std::fs::remove_dir_all(config.folder).unwrap();
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
        tx.send((
            capture::Packet {
                frame: 19,
                samples: vec![0.4; 12],
            },
            capture::TimingEvent::None,
        ))
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
