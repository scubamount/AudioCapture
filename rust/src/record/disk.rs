//! A recording owns one bounded disk queue. The same thread opens, writes,
//! and closes the codecs, including the Media Foundation COM apartment.
use super::*;
use intervals::{DiskFiles, Mapping};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, SyncSender};

type Slab = Box<[f32; BLOCK as usize * 2]>;
const WRITER_READY: u8 = 1;
const WRITER_FAILED: u8 = 2;
static OWNERS: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        OWNERS.fetch_sub(1, Ordering::AcqRel);
    }
}
enum Event {
    Audio(Option<usize>, usize, Slab, bool),
    #[cfg(test)]
    Hold(SyncSender<()>, Receiver<()>),
    #[cfg(test)]
    Fail,
    Marker(u64, String),
    Enable(String, u64, u64),
    Span(u64, u64, u64, Mapping),
    Overlap(String, u64, u64),
    Gap(usize, u64, u64, &'static str),
    Uncertain(usize, u64),
}
struct Sender {
    tx: SyncSender<Event>,
    pool: Receiver<Slab>,
    folder: PathBuf,
    final_drain: Cell<bool>,
}
impl Sender {
    fn event(&self, event: Event) -> Result<(), String> {
        let mut item = event;
        let mut deadline = None;
        loop {
            match self.tx.try_send(item) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err(format!(
                        "Recording writer stopped: {}",
                        self.folder.display()
                    ));
                }
                Err(mpsc::TrySendError::Full(event)) => {
                    if !self.final_drain.get() {
                        return Err(format!(
                            "Recording disk queue full: {}",
                            self.folder.display()
                        ));
                    }
                    if Instant::now()
                        >= *deadline.get_or_insert_with(|| Instant::now() + STOP_GRACE)
                    {
                        return Err(format!(
                            "Recording final drain timed out: {}",
                            self.folder.display()
                        ));
                    }
                    item = event;
                    thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }
}

pub(super) struct Proxy {
    owner: Rc<Sender>,
    index: Option<usize>,
    last: bool,
}
impl Proxy {
    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        let mut buffer = match self.owner.pool.try_recv() {
            Ok(buffer) => buffer,
            Err(mpsc::TryRecvError::Empty) if self.owner.final_drain.get() => {
                // Every capture source has already exited and the live inlet
                // has sealed. Backpressure here preserves the last accepted block.
                self.owner.pool.recv_timeout(STOP_GRACE).map_err(|_| {
                    format!(
                        "Recording final drain timed out: {}",
                        self.owner.folder.display()
                    )
                })?
            }
            Err(_) => {
                return Err(format!(
                    "Recording writer could not keep up: {}",
                    self.owner.folder.display()
                ));
            }
        };
        buffer[..samples.len()].copy_from_slice(samples);
        self.owner
            .event(Event::Audio(self.index, samples.len(), buffer, self.last))
    }
}
pub(super) struct Map(Rc<Sender>);
impl Map {
    pub fn block(
        &mut self,
        first: u64,
        mut saved: u64,
        keep: &BlockSelection,
    ) -> Result<(), String> {
        if keep.kept == keep.valid {
            return self.0.event(Event::Span(
                first,
                first + keep.valid as u64,
                saved,
                Mapping::Saved,
            ));
        }
        if keep.kept == 0 {
            return self.0.event(Event::Span(
                first,
                first + keep.valid as u64,
                saved,
                if keep.audible {
                    Mapping::Pause
                } else {
                    Mapping::Silence
                },
            ));
        }
        let mut offset = 0;
        while offset < keep.valid {
            let retained = keep.mask[offset];
            let n = keep.mask[offset..keep.valid]
                .iter()
                .take_while(|&&v| v == retained)
                .count();
            self.0.event(Event::Span(
                first + offset as u64,
                first + (offset + n) as u64,
                saved,
                if retained {
                    Mapping::Saved
                } else {
                    Mapping::Pause
                },
            ))?;
            if retained {
                saved += n as u64
            }
            offset += n;
        }
        Ok(())
    }
    pub fn source_gap(
        &mut self,
        source: usize,
        start: u64,
        end: u64,
        cause: &'static str,
    ) -> Result<(), String> {
        self.0.event(Event::Gap(source, start, end, cause))
    }
    pub fn uncertain(&mut self, source: usize, frame: u64) -> Result<(), String> {
        self.0.event(Event::Uncertain(source, frame))
    }
    pub fn overlap(&mut self, id: &str, start: u64, end: u64) -> Result<(), String> {
        self.0.event(Event::Overlap(id.into(), start, end))
    }
}
pub(super) trait MarkerSink {
    fn mark(&mut self, frame: u64, label: &str) -> Result<(), String>;
}
impl MarkerSink for markers::MarkerWriter {
    fn mark(&mut self, frame: u64, label: &str) -> Result<(), String> {
        markers::MarkerWriter::mark(self, frame, label)
    }
}
pub(super) struct MarkerProxy(Rc<Sender>);
impl MarkerSink for MarkerProxy {
    fn mark(&mut self, frame: u64, label: &str) -> Result<(), String> {
        self.0.event(Event::Marker(frame, label.into()))
    }
}

pub(super) struct Recording {
    pub id: String,
    pub first: u64,
    pub written: u64,
    pub incomplete: bool,
    pub files: Vec<Option<Proxy>>,
    pub mixed: Option<Proxy>,
    pub markers: MarkerProxy,
    pub map: Option<Map>,
    owner: Rc<Sender>,
    description: String,
    pub first_path: PathBuf,
    state: Arc<std::sync::atomic::AtomicU8>,
    finish: Arc<Mutex<Option<(u64, u64, bool)>>>,
    worker: JoinHandle<Result<(), String>>,
}
impl Recording {
    pub fn open(config: &Config, first: u64) -> Result<Self, String> {
        if !config.separate && !config.mixed {
            return Err("Select at least one recording output".into());
        }
        if OWNERS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 8).then_some(n + 1)
            })
            .is_err()
        {
            return Err("Too many recordings are still finalizing".into());
        }
        let permit = Permit;
        let id = next_id();
        let outputs =
            usize::from(config.mixed) + usize::from(config.separate) * config.inputs.len();
        // The mixer releases its 200 ms alignment window as a burst at Stop.
        // One second per output also covers ordinary scheduler preemption.
        // Two separate mono Opus stems reserve 750 KiB of stereo input slabs.
        let count = (outputs * (RATE / BLOCK) as usize).clamp(1, 4096);
        let (tx, rx) = mpsc::sync_channel(count * 2 + 32);
        let (recycle, pool) = mpsc::sync_channel(count);
        for _ in 0..count {
            recycle
                .try_send(Box::new([0.0; BLOCK as usize * 2]))
                .unwrap();
        }
        let finish = Arc::new(Mutex::new(None));
        let done = Arc::clone(&finish);
        let state = Arc::new(std::sync::atomic::AtomicU8::new(0));
        let worker_state = state.clone();
        let target = config.clone();
        let worker = thread::Builder::new()
            .name("recording-file-writer".into())
            .spawn(move || {
                let _permit = permit;
                run(&target, first, id, rx, recycle, done, worker_state)
            })
            .map_err(|e| format!("Cannot start recording writer: {e}"))?;
        let owner = Rc::new(Sender {
            tx,
            pool,
            folder: config.folder.clone(),
            final_drain: Cell::new(false),
        });
        let files = (0..config.inputs.len())
            .map(|i| {
                config.separate.then(|| Proxy {
                    owner: owner.clone(),
                    index: Some(i),
                    last: !config.mixed && i + 1 == config.inputs.len(),
                })
            })
            .collect();
        let mixed = config.mixed.then(|| Proxy {
            owner: owner.clone(),
            index: None,
            last: true,
        });
        let mut paths = Vec::new();
        if config.separate {
            for (i, (name, _, _)) in config.inputs.iter().enumerate() {
                paths.push(output_path(config, &format!("{name}-{}", i + 1), id));
            }
        }
        if config.mixed {
            paths.push(output_path(config, "Combined", id));
        }
        let description = paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(" | ");
        Ok(Self {
            id: id.to_string(),
            first,
            written: 0,
            incomplete: false,
            files,
            mixed,
            markers: MarkerProxy(owner.clone()),
            map: None,
            owner,
            description,
            first_path: paths.remove(0),
            state,
            finish,
            worker,
        })
    }
    #[cfg(test)]
    pub fn stall(&self) -> (Receiver<()>, SyncSender<()>) {
        let (entered, waiting) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        self.owner.event(Event::Hold(entered, resume)).unwrap();
        (waiting, release)
    }
    #[cfg(test)]
    pub fn inject_failure(&self) {
        self.owner.event(Event::Fail).unwrap();
    }
    pub fn allow_final_drain(&self) {
        self.owner.final_drain.set(true);
    }
    pub fn describe(&self) -> String {
        self.description.clone()
    }
    pub fn enable_map(&mut self, _: &Config, session: &str, frame: u64) -> Result<(), String> {
        if self.map.is_none() {
            self.owner
                .event(Event::Enable(session.into(), frame, self.written))?;
            self.map = Some(Map(self.owner.clone()));
        }
        Ok(())
    }
    pub fn ready(&self) -> bool {
        self.state.load(Ordering::Acquire) == WRITER_READY
    }
    pub fn closed(&self) -> bool {
        self.state.load(Ordering::Acquire) == WRITER_FAILED || self.worker.is_finished()
    }
    pub fn finalize(self, end: u64, healthy: bool) -> JoinHandle<Result<(), String>> {
        *self.finish.lock().unwrap() = Some((end, self.written, healthy && !self.incomplete));
        // All sender clones drop on return. The owner drains accepted work
        // then reads the cutoff even when the bounded media queue was full.
        self.worker
    }
}

fn run(
    config: &Config,
    first: u64,
    id: u128,
    rx: Receiver<Event>,
    recycle: SyncSender<Slab>,
    done: Arc<Mutex<Option<(u64, u64, bool)>>>,
    state: Arc<std::sync::atomic::AtomicU8>,
) -> Result<(), String> {
    let mut r = DiskFiles::open(config, first, id)?;
    state.store(WRITER_READY, Ordering::Release);
    let mut failure = None;
    let mut metadata_error = None;
    let mut pending = Vec::with_capacity(config.inputs.len() + 1);
    let mut persisted = 0u64;
    for event in rx {
        if failure.is_some() {
            continue;
        }
        let result = match event {
            Event::Audio(idx, n, samples, last) => {
                pending.push((idx, n, samples));
                if !last {
                    continue;
                }
                // A full producer queue can interrupt a block between two stems.
                // Commit a batch only after the final stem was admitted.
                let mut result = Ok(());
                for (idx, n, samples) in pending.drain(..) {
                    let writer = match idx {
                        Some(i) => r.files[i].as_mut(),
                        None => r.mixed.as_mut(),
                    };
                    if result.is_ok() {
                        result = writer
                            .ok_or_else(|| "Unexpected recording output index".to_string())
                            .and_then(|w| w.write(&samples[..n]));
                    }
                    let _ = recycle.try_send(samples);
                }
                if result.is_ok() {
                    persisted += (n / 2) as u64;
                }
                result
            }
            #[cfg(test)]
            Event::Hold(entered, release) => {
                let _ = entered.send(());
                let _ = release.recv();
                Ok(())
            }
            #[cfg(test)]
            Event::Fail => Err("Injected disk failure".into()),
            Event::Marker(at, label) => {
                if let Err(e) = r.markers.mark(at, &label) {
                    metadata_error.get_or_insert(format!("Marker: {e}"));
                }
                Ok(())
            }
            entry => {
                let result = match entry {
                    Event::Enable(session, at, saved) => {
                        r.written = saved;
                        r.enable_map(config, &session, at)
                    }
                    event => {
                        if let Some(map) = &mut r.map {
                            match event {
                                Event::Span(a, b, saved, kind) => map.span(a, b, saved, kind),
                                Event::Overlap(id, a, b) => map.overlap(&id, a, b),
                                Event::Gap(i, a, b, cause) => map.source_gap(i, a, b, cause),
                                Event::Uncertain(i, at) => map.uncertain(i, at),
                                _ => unreachable!(),
                            }
                        } else {
                            Ok(())
                        }
                    }
                };
                if let Err(e) = result {
                    // A failed sidecar cannot stop a healthy encoder or the live
                    // consumer. Leave the partial sidecar available for inspection.
                    r.map.take();
                    metadata_error.get_or_insert(format!("Recording timeline: {e}"));
                }
                Ok(())
            }
        };
        if let Err(error) = result {
            failure = Some(error);
            state.store(WRITER_FAILED, Ordering::Release);
        }
    }
    let (end, saved, healthy) = done.lock().unwrap().unwrap_or((first, 0, false));
    r.written = if healthy && failure.is_none() {
        saved
    } else {
        persisted
    };
    let finalized = r.finalize(config, end, healthy && failure.is_none());
    failure.map_or(finalized, Err)?;
    if let Some(error) = metadata_error {
        return Err(format!("Audio files saved. {error}"));
    }
    if !healthy {
        return Err("Recording ended before all output could be saved".into());
    }
    Ok(())
}
