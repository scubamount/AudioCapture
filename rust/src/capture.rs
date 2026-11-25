//! WASAPI capture streams for a single selected source.
//!
//! Every stream requests stereo 48 kHz float samples. WASAPI performs any
//! device conversion. Application capture includes the selected process tree.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use wasapi::{AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

const SAMPLE_RATE: u64 = 48_000;
const BYTES_PER_FRAME: usize = 8;
const HNS_PER_SECOND: u64 = 10_000_000;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const SYNCHRONIZE: u32 = 0x0010_0000;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
static PROCESS_ACTIVATION_BUSY: AtomicBool = AtomicBool::new(false);

// The upstream process-loopback activation can block indefinitely. Serialize
// only activation so one blocked activation cannot create unbounded workers.
// Waiters observe session cancellation, and the permit stays held by a stuck
// activation until Windows returns.
struct ActivationPermit<'a>(&'a AtomicBool);

impl<'a> ActivationPermit<'a> {
    fn acquire(busy: &'a AtomicBool, stop: &AtomicBool) -> Result<Self, String> {
        loop {
            if stop.load(Ordering::Acquire) {
                return Err("Capture stopped before process activation".into());
            }
            if busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(Self(busy));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for ActivationPermit<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[repr(C)]
struct FileTime {
    low: u32,
    high: u32,
}

impl FileTime {
    fn ticks(&self) -> u64 {
        (u64::from(self.high) << 32) | u64::from(self.low)
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryPerformanceCounter(value: *mut i64) -> i32;
    fn QueryPerformanceFrequency(value: *mut i64) -> i32;
    fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut std::ffi::c_void;
    fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    fn GetProcessTimes(
        handle: *mut std::ffi::c_void,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
}

#[derive(Clone, Debug)]
pub enum Input {
    Process(u32),
    SelectedProcess {
        pid: u32,
        created: u64,
    },
    System,
    /// A specific Windows render endpoint, resolved before capture starts.
    SystemEndpoint(String),
    /// The Windows endpoint ID returned by the device enumerator.
    Mic(String),
}

#[derive(Debug)]
pub struct Packet {
    /// Index of the first frame relative to the shared session epoch.
    pub frame: u64,
    /// Interleaved stereo floating-point PCM at 48 kHz.
    pub samples: Vec<f32>,
}

/// Timing observations for one capture source. A known gap is measured from
/// valid packet timestamps. Other flagged events have an unknown extent.
#[derive(Default)]
pub struct CaptureStats {
    pub known_gap_events: AtomicU64,
    pub known_gap_frames: AtomicU64,
    pub uncertain_events: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TimingEvent {
    None,
    KnownGap(u64),
    Uncertain,
}

enum PacketSink {
    Plain(SyncSender<Packet>),
    Timed(SyncSender<(Packet, TimingEvent)>),
}

impl PacketSink {
    fn send(&self, packet: Packet, timing: TimingEvent) -> Result<(), String> {
        fn checked<T>(result: Result<(), TrySendError<T>>) -> Result<(), String> {
            match result {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(_)) => {
                    Err("Audio capture queue overflowed; recording is incomplete".into())
                }
                Err(TrySendError::Disconnected(_)) => {
                    Err("Audio recording worker disconnected".into())
                }
            }
        }
        match self {
            Self::Plain(tx) => checked(tx.try_send(packet)),
            Self::Timed(tx) => checked(tx.try_send((packet, timing))),
        }
    }
}

fn add_saturating(counter: &AtomicU64, amount: u64) {
    let mut count = counter.load(Ordering::Relaxed);
    while count != u64::MAX {
        match counter.compare_exchange_weak(
            count,
            count.saturating_add(amount),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => count = actual,
        }
    }
}

impl CaptureStats {
    fn record(&self, event: TimingEvent) {
        match event {
            TimingEvent::None => {}
            TimingEvent::KnownGap(frames) => {
                add_saturating(&self.known_gap_events, 1);
                add_saturating(&self.known_gap_frames, frames);
            }
            TimingEvent::Uncertain => add_saturating(&self.uncertain_events, 1),
        }
    }
}

struct ComApartment;

impl ComApartment {
    fn new() -> Result<Self, String> {
        wasapi::initialize_mta()
            .ok()
            .map_err(|error| format!("COM initialization failed: {error}"))?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        wasapi::deinitialize();
    }
}

struct ProcessHandle(*mut std::ffi::c_void);

impl ProcessHandle {
    fn open(pid: u32) -> Result<Self, String> {
        let handle =
            unsafe { OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return Err(format!(
                "Cannot monitor process {pid}: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self(handle))
    }

    fn creation_time(&self) -> Result<u64, String> {
        let mut creation = FileTime { low: 0, high: 0 };
        let mut exit = FileTime { low: 0, high: 0 };
        let mut kernel = FileTime { low: 0, high: 0 };
        let mut user = FileTime { low: 0, high: 0 };
        let ok =
            unsafe { GetProcessTimes(self.0, &mut creation, &mut exit, &mut kernel, &mut user) };
        if ok == 0 {
            return Err(format!(
                "Cannot read selected process identity: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(creation.ticks())
    }

    fn verify_identity(&self, expected: u64) -> Result<(), String> {
        self.check_alive()?;
        if self.creation_time()? != expected {
            return Err(
                "Selected process changed since selection; refresh and select it again".into(),
            );
        }
        Ok(())
    }

    fn check_alive(&self) -> Result<(), String> {
        match unsafe { WaitForSingleObject(self.0, 0) } {
            WAIT_TIMEOUT => Ok(()),
            WAIT_OBJECT_0 => Err("Selected process exited during capture".into()),
            _ => Err(format!(
                "Cannot check selected process: {}",
                std::io::Error::last_os_error()
            )),
        }
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Start and run one capture stream on its own thread.
///
/// The ready channel receives exactly one result. The returned result reports
/// runtime failures, including queue overflow, device loss, and source exit.
pub fn run(
    input: Input,
    sink: SyncSender<Packet>,
    stop: Arc<AtomicBool>,
    ready: Sender<Result<(), String>>,
    epoch: Instant,
) -> Result<(), String> {
    run_with_counters(
        input,
        PacketSink::Plain(sink),
        stop,
        ready,
        epoch,
        &CaptureStats::default(),
        None,
    )
}

/// Run capture and count timing events that can make its timeline uncertain.
///
/// The count does not represent the number of lost frames. A discontinuity
/// can indicate loss without revealing how many frames are missing.
pub fn run_observed(
    input: Input,
    sink: SyncSender<Packet>,
    stop: Arc<AtomicBool>,
    ready: Sender<Result<(), String>>,
    epoch: Instant,
    glitches: Arc<AtomicU64>,
) -> Result<(), String> {
    run_with_counters(
        input,
        PacketSink::Plain(sink),
        stop,
        ready,
        epoch,
        &CaptureStats::default(),
        Some(&glitches),
    )
}

/// Run capture and publish separate measured-gap and uncertain-timing counts.
pub fn run_with_stats(
    input: Input,
    sink: SyncSender<Packet>,
    stop: Arc<AtomicBool>,
    ready: Sender<Result<(), String>>,
    epoch: Instant,
    stats: Arc<CaptureStats>,
) -> Result<(), String> {
    run_with_counters(
        input,
        PacketSink::Plain(sink),
        stop,
        ready,
        epoch,
        &stats,
        None,
    )
}

/// Keep discontinuity evidence attached to the packet that established it.
/// This shares the existing bounded audio queue and its ordering.
pub(crate) fn run_timed(
    input: Input,
    sink: SyncSender<(Packet, TimingEvent)>,
    stop: Arc<AtomicBool>,
    ready: Sender<Result<(), String>>,
    epoch: Instant,
    stats: Arc<CaptureStats>,
) -> Result<(), String> {
    run_with_counters(
        input,
        PacketSink::Timed(sink),
        stop,
        ready,
        epoch,
        &stats,
        None,
    )
}

fn run_with_counters(
    input: Input,
    sink: PacketSink,
    stop: Arc<AtomicBool>,
    ready: Sender<Result<(), String>>,
    epoch: Instant,
    stats: &CaptureStats,
    legacy_events: Option<&AtomicU64>,
) -> Result<(), String> {
    let mut ready = Some(ready);
    let result = run_inner(input, sink, stop, &mut ready, epoch, stats, legacy_events);
    if let Some(ready) = ready {
        let message = result
            .as_ref()
            .err()
            .cloned()
            .unwrap_or_else(|| "Capture stopped before the stream was ready".to_string());
        let _ = ready.send(Err(message));
    }
    result
}

fn run_inner(
    input: Input,
    sink: PacketSink,
    stop: Arc<AtomicBool>,
    ready: &mut Option<Sender<Result<(), String>>>,
    epoch: Instant,
    stats: &CaptureStats,
    legacy_events: Option<&AtomicU64>,
) -> Result<(), String> {
    let _apartment = ComApartment::new()?;
    if stop.load(Ordering::Acquire) {
        return Err("Capture stopped before activation".into());
    }
    let process = match input {
        Input::Process(pid) | Input::SelectedProcess { pid, .. } => Some(ProcessHandle::open(pid)?),
        _ => None,
    };
    if let (Input::SelectedProcess { created, .. }, Some(handle)) = (&input, &process) {
        handle.verify_identity(*created)?;
    }
    let mut client = match &input {
        Input::Process(pid) | Input::SelectedProcess { pid, .. } => {
            let _permit = ActivationPermit::acquire(&PROCESS_ACTIVATION_BUSY, &stop)?;
            if stop.load(Ordering::Acquire) {
                return Err("Capture stopped before process activation".into());
            }
            AudioClient::new_application_loopback_client(*pid, true)
                .map_err(|e| format!("Cannot activate application capture for PID {pid}: {e}"))?
        }
        Input::System => DeviceEnumerator::new()
            .and_then(|enumerator| enumerator.get_default_device(Direction::Render))
            .and_then(|device| device.get_iaudioclient())
            .map_err(|e| format!("Cannot open the system playback device: {e}"))?,
        Input::SystemEndpoint(id) => DeviceEnumerator::new()
            .and_then(|enumerator| enumerator.get_device(id))
            .and_then(|device| device.get_iaudioclient())
            .map_err(|e| format!("Cannot open system playback device {id}: {e}"))?,
        Input::Mic(id) => DeviceEnumerator::new()
            .and_then(|enumerator| enumerator.get_device(id))
            .and_then(|device| device.get_iaudioclient())
            .map_err(|e| format!("Cannot open microphone {id}: {e}"))?,
    };

    if stop.load(Ordering::Acquire) {
        return Err("Capture stopped during activation".into());
    }
    let format = WaveFormat::new(32, 32, SampleType::Float, SAMPLE_RATE as usize, 2, None);
    client
        .initialize_client(
            &format,
            Direction::Capture,
            StreamMode::EventsShared {
                autoconvert: true,
                buffer_duration_hns: 0,
            },
            None,
        )
        .map_err(|e| format!("Cannot initialize capture stream: {e}"))?;
    let event = client
        .set_get_eventhandle()
        .map_err(|e| format!("Cannot create capture event: {e}"))?;
    let capture = client
        .get_audiocaptureclient()
        .map_err(|e| format!("Cannot open capture service: {e}"))?;
    let epoch_hns = performance_counter_hns()?
        .saturating_sub(epoch.elapsed().as_nanos().saturating_div(100) as u64);
    if stop.load(Ordering::Acquire) {
        return Err("Capture stopped before the stream started".into());
    }
    client
        .start_stream()
        .map_err(|e| format!("Cannot start capture stream: {e}"))?;
    if stop.load(Ordering::Acquire) {
        let _ = client.stop_stream();
        return Err("Capture stopped during stream startup".into());
    }
    if let Some(process) = &process {
        if let Err(error) = process.check_alive() {
            let _ = client.stop_stream();
            return Err(error);
        }
    }
    if let Some(ready) = ready.take() {
        if ready.send(Ok(())).is_err() {
            let _ = client.stop_stream();
            return Err("Capture coordinator disconnected during startup".into());
        }
    }

    let result = (|| {
        let mut bytes = Vec::new();
        let mut next_frame = None;
        let mut had_packet = false;
        loop {
            if !stop.load(Ordering::Acquire) {
                if let Some(process) = &process {
                    process.check_alive()?;
                }
            }
            loop {
                let frames = capture
                    .get_next_packet_size()
                    .map_err(|e| format!("Cannot query captured audio: {e}"))?
                    .unwrap_or(0) as usize;
                if frames == 0 {
                    break;
                }
                bytes.resize(frames * BYTES_PER_FRAME, 0);
                let (read, info) = capture
                    .read_from_device(&mut bytes)
                    .map_err(|e| format!("Cannot read captured audio: {e}"))?;
                if read == 0 {
                    break;
                }
                let elapsed_frames = if next_frame.is_none()
                    && (info.flags.timestamp_error
                        || info.timestamp == 0
                        || info.timestamp < epoch_hns)
                {
                    (epoch
                        .elapsed()
                        .as_nanos()
                        .saturating_mul(SAMPLE_RATE as u128)
                        / 1_000_000_000)
                        .min(u64::MAX as u128) as u64
                } else {
                    0
                };
                let (frame, event) = packet_frame(
                    info.timestamp,
                    info.flags.timestamp_error,
                    info.flags.data_discontinuity,
                    had_packet,
                    epoch_hns,
                    next_frame,
                    elapsed_frames,
                    read as u64,
                );
                if event != TimingEvent::None {
                    if let Some(events) = legacy_events {
                        add_saturating(events, 1);
                    }
                    stats.record(event);
                }
                let samples = bytes[..read as usize * BYTES_PER_FRAME]
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect();
                sink.send(Packet { frame, samples }, event)?;
                next_frame = Some(frame.saturating_add(read as u64));
                had_packet = true;
            }
            if stop.load(Ordering::Acquire) {
                break;
            }
            match event.wait_for_event(100) {
                Ok(()) | Err(wasapi::WasapiError::EventTimeout) => {}
                Err(error) => return Err(format!("Capture event failed: {error}")),
            }
        }
        Ok(())
    })();
    let stop_result = client
        .stop_stream()
        .map_err(|e| format!("Cannot stop capture stream: {e}"));
    result.and(stop_result)
}

/// Prefer the clock timestamp when Windows reports one as valid. Without a
/// reliable timestamp, retain sample order as an estimate and report that the
/// actual gap is unknown. The elapsed-time estimate is only used at startup.
fn packet_frame(
    timestamp: u64,
    timestamp_error: bool,
    discontinuity: bool,
    had_packet: bool,
    epoch_hns: u64,
    next_frame: Option<u64>,
    elapsed_frames: u64,
    read_frames: u64,
) -> (u64, TimingEvent) {
    let valid_timestamp = !timestamp_error && timestamp != 0 && timestamp >= epoch_hns;
    let frame = if valid_timestamp {
        timestamp
            .saturating_sub(epoch_hns)
            .saturating_mul(SAMPLE_RATE)
            / HNS_PER_SECOND
    } else {
        next_frame.unwrap_or_else(|| elapsed_frames.saturating_sub(read_frames))
    };
    let event = if !valid_timestamp {
        TimingEvent::Uncertain
    } else if discontinuity && had_packet {
        match next_frame.and_then(|previous| frame.checked_sub(previous)) {
            Some(gap) if gap > 0 => TimingEvent::KnownGap(gap),
            _ => TimingEvent::Uncertain,
        }
    } else {
        TimingEvent::None
    };
    (frame, event)
}

fn performance_counter_hns() -> Result<u64, String> {
    let mut counter = 0;
    let mut frequency = 0;
    if unsafe { QueryPerformanceCounter(&mut counter) } == 0
        || unsafe { QueryPerformanceFrequency(&mut frequency) } == 0
        || frequency <= 0
        || counter < 0
    {
        return Err(format!(
            "Cannot read the Windows performance clock: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(((counter as u128 * HNS_PER_SECOND as u128) / frequency as u128) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_process_identity_rejects_changed_creation_time() {
        let handle = ProcessHandle::open(std::process::id()).unwrap();
        let created = handle.creation_time().unwrap();
        assert!(handle.verify_identity(created).is_ok());
        assert!(handle.verify_identity(created.wrapping_add(1)).is_err());
    }

    #[test]
    fn discontinuity_keeps_a_valid_clock_gap_and_reports_measured_loss() {
        // 125 ms after the epoch, after a previous packet ending at 100 ms.
        let (frame, event) = packet_frame(
            2_250_000,
            false,
            true,
            true,
            1_000_000,
            Some(4_800),
            6_000,
            480,
        );
        assert_eq!(frame, 6_000);
        assert_eq!(event, TimingEvent::KnownGap(1_200));
    }

    #[test]
    fn unusable_clock_continues_in_order_and_reports_unknown_gap() {
        let (frame, event) = packet_frame(
            9_999_999,
            true,
            true,
            true,
            1_000_000,
            Some(4_800),
            12_000,
            480,
        );
        assert_eq!(frame, 4_800);
        assert_eq!(event, TimingEvent::Uncertain);
        let (initial, initial_event) =
            packet_frame(0, true, false, false, 1_000_000, None, 600, 480);
        assert_eq!(initial, 120);
        assert_eq!(initial_event, TimingEvent::Uncertain);
    }

    #[test]
    fn initial_discontinuity_flag_does_not_imply_lost_audio() {
        let (frame, event) = packet_frame(1_100_000, false, true, false, 1_000_000, None, 480, 480);
        assert_eq!(frame, 480);
        assert_eq!(event, TimingEvent::None);
        let (bad_frame, bad_event) = packet_frame(
            900_000,
            false,
            false,
            true,
            1_000_000,
            Some(960),
            1_000,
            480,
        );
        assert_eq!(bad_frame, 960);
        assert_eq!(bad_event, TimingEvent::Uncertain);
    }

    #[test]
    fn timing_counters_distinguish_gaps_and_unknown_events() {
        let stats = CaptureStats::default();
        stats.record(TimingEvent::KnownGap(1200));
        stats.record(TimingEvent::Uncertain);
        stats.record(TimingEvent::None);
        assert_eq!(stats.known_gap_events.load(Ordering::Relaxed), 1);
        assert_eq!(stats.known_gap_frames.load(Ordering::Relaxed), 1200);
        assert_eq!(stats.uncertain_events.load(Ordering::Relaxed), 1);
        stats
            .known_gap_frames
            .store(u64::MAX - 2, Ordering::Relaxed);
        stats.record(TimingEvent::KnownGap(3));
        assert_eq!(stats.known_gap_frames.load(Ordering::Relaxed), u64::MAX);
    }

    #[test]
    fn cancellation_stops_waiters_without_releasing_stuck_activation() {
        let busy = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let permit = ActivationPermit::acquire(&busy, &stop).unwrap();
        let waiter_busy = Arc::clone(&busy);
        let waiter_stop = Arc::clone(&stop);
        let waiter = std::thread::spawn(move || {
            ActivationPermit::acquire(&waiter_busy, &waiter_stop).map(|_| ())
        });
        stop.store(true, Ordering::Release);
        assert!(waiter.join().unwrap().is_err());
        assert!(busy.load(Ordering::Acquire));
        drop(permit);
        assert!(!busy.load(Ordering::Acquire));
    }

    #[test]
    fn discontinuity_without_a_positive_clock_gap_is_uncertain() {
        let (frame, event) = packet_frame(
            2_000_000,
            false,
            true,
            true,
            1_000_000,
            Some(4_800),
            4_800,
            480,
        );
        assert_eq!(frame, 4_800);
        assert_eq!(event, TimingEvent::Uncertain);
    }
}
