use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

const MAX_PENDING_SAMPLES: usize = 96_000;

enum Command {
    Samples(Vec<f32>),
}

pub struct Monitor {
    sender: SyncSender<Command>,
    errors: Receiver<String>,
    worker: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

impl Monitor {
    pub fn open(endpoint_id: &str) -> Result<Self, String> {
        let endpoint_id = endpoint_id.to_owned();
        let (sender, commands) = mpsc::sync_channel(32);
        let (error_sender, errors) = mpsc::channel();
        let (ready_sender, ready) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            if let Err(error) = run(&endpoint_id, commands, &ready_sender, &worker_stop) {
                let _ = ready_sender.send(Err(error.clone()));
                let _ = error_sender.send(error);
            }
        });
        match ready.recv() {
            Ok(Ok(())) => Ok(Self {
                sender,
                errors,
                worker: Some(worker),
                stop,
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                Err("Monitor thread stopped during initialization".to_string())
            }
        }
    }

    // This never waits for the audio endpoint. The caller can disable monitoring
    // while recording continues if the queue or the endpoint fails.
    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        if let Ok(error) = self.errors.try_recv() {
            return Err(error);
        }
        if samples.len() % 2 != 0 || samples.len() > MAX_PENDING_SAMPLES {
            return Err("Invalid monitor buffer size".to_string());
        }
        match self.sender.try_send(Command::Samples(samples.to_vec())) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("Monitor queue is full".to_string()),
            Err(TrySendError::Disconnected(_)) => Err(self
                .errors
                .try_recv()
                .unwrap_or_else(|_| "Monitor stopped".to_string())),
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(
    endpoint_id: &str,
    commands: Receiver<Command>,
    ready: &mpsc::Sender<Result<(), String>>,
    stop: &AtomicBool,
) -> Result<(), String> {
    wasapi::initialize_mta()
        .ok()
        .map_err(|e| format!("Monitor initialization failed: {e}"))?;
    let result = run_initialized(endpoint_id, commands, ready, stop);
    wasapi::deinitialize();
    result
}

fn run_initialized(
    endpoint_id: &str,
    commands: Receiver<Command>,
    ready: &mpsc::Sender<Result<(), String>>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let enumerator = DeviceEnumerator::new().map_err(|e| e.to_string())?;
    let device = enumerator
        .get_device(endpoint_id)
        .map_err(|e| format!("Monitor device unavailable: {e}"))?;
    let mut client = device.get_iaudioclient().map_err(|e| e.to_string())?;
    let format = WaveFormat::new(32, 32, SampleType::Float, 48_000, 2, None);
    let (default_period, _) = client.get_device_period().map_err(|e| e.to_string())?;
    client
        .initialize_client(
            &format,
            Direction::Render,
            StreamMode::PollingShared {
                autoconvert: true,
                buffer_duration_hns: default_period * 4,
            },
            None,
        )
        .map_err(|e| format!("Cannot open monitor device: {e}"))?;
    let render = client.get_audiorenderclient().map_err(|e| e.to_string())?;
    client
        .start_stream()
        .map_err(|e| format!("Cannot start monitor: {e}"))?;
    let _ = ready.send(Ok(()));
    let mut pending = VecDeque::new();
    loop {
        if stop.load(Ordering::Acquire) {
            let _ = client.stop_stream();
            return Ok(());
        }
        loop {
            match commands.try_recv() {
                Ok(Command::Samples(samples)) => {
                    if pending.len() + samples.len() > MAX_PENDING_SAMPLES {
                        return Err("Monitor could not keep up with playback".to_string());
                    }
                    pending.extend(samples);
                }
                Err(TryRecvError::Disconnected) => {
                    let _ = client.stop_stream();
                    return Ok(());
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        let frames = client
            .get_available_space_in_frames()
            .map_err(|e| format!("Monitor device unavailable: {e}"))? as usize;
        if frames > 0 {
            let mut bytes = Vec::with_capacity(frames * 8);
            for _ in 0..frames * 2 {
                bytes.extend_from_slice(&pending.pop_front().unwrap_or(0.0f32).to_le_bytes());
            }
            render
                .write_to_device(&bytes, None)
                .map_err(|e| format!("Monitor playback failed: {e}"))?;
        }
        thread::sleep(Duration::from_millis(5));
    }
}
