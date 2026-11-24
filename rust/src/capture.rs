//! WASAPI capture streams for a single selected source.
//!
//! Every stream requests stereo 48 kHz float samples. WASAPI performs any
//! device conversion. Application capture includes the selected process tree.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, SyncSender, TrySendError};
use std::time::Instant;

use wasapi::{AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

const SAMPLE_RATE: u64 = 48_000;
const BYTES_PER_FRAME: usize = 8;
const HNS_PER_SECOND: u64 = 10_000_000;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const SYNCHRONIZE: u32 = 0x0010_0000;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

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
    let mut ready = Some(ready);
    let result = run_inner(input, sink, stop, &mut ready, epoch);
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
    sink: SyncSender<Packet>,
    stop: Arc<AtomicBool>,
    ready: &mut Option<Sender<Result<(), String>>>,
    epoch: Instant,
) -> Result<(), String> {
    let _apartment = ComApartment::new()?;
    let process = match input {
        Input::Process(pid) | Input::SelectedProcess { pid, .. } => Some(ProcessHandle::open(pid)?),
        _ => None,
    };
    if let (Input::SelectedProcess { created, .. }, Some(handle)) = (&input, &process) {
        handle.verify_identity(*created)?;
    }
    let mut client = match &input {
        Input::Process(pid) | Input::SelectedProcess { pid, .. } => {
            AudioClient::new_application_loopback_client(*pid, true)
                .map_err(|e| format!("Cannot activate application capture for PID {pid}: {e}"))?
        }
        Input::System => DeviceEnumerator::new()
            .and_then(|enumerator| enumerator.get_default_device(Direction::Render))
            .and_then(|device| device.get_iaudioclient())
            .map_err(|e| format!("Cannot open the system playback device: {e}"))?,
        Input::Mic(id) => DeviceEnumerator::new()
            .and_then(|enumerator| enumerator.get_device(id))
            .and_then(|device| device.get_iaudioclient())
            .map_err(|e| format!("Cannot open microphone {id}: {e}"))?,
    };

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
    client
        .start_stream()
        .map_err(|e| format!("Cannot start capture stream: {e}"))?;
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
                if info.flags.data_discontinuity && had_packet {
                    return Err("Audio capture dropped frames".into());
                }
                let frame = if !info.flags.timestamp_error && info.timestamp != 0 {
                    info.timestamp
                        .saturating_sub(epoch_hns)
                        .saturating_mul(SAMPLE_RATE)
                        / HNS_PER_SECOND
                } else if let Some(frame) = next_frame {
                    frame
                } else {
                    let now = epoch
                        .elapsed()
                        .as_nanos()
                        .saturating_mul(SAMPLE_RATE as u128)
                        / 1_000_000_000;
                    (now as u64).saturating_sub(read as u64)
                };
                let samples = bytes[..read as usize * BYTES_PER_FRAME]
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect();
                match sink.try_send(Packet { frame, samples }) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        return Err(
                            "Audio capture queue overflowed; recording is incomplete".into()
                        );
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        return Err("Audio recording worker disconnected".into());
                    }
                }
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
}
