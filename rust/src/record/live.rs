use super::*;
use std::sync::atomic::AtomicU64;

/// A bounded producer connected to the speech worker. Implementations must
/// not wait, allocate per block, access disk or perform IPC on this thread.
pub trait LiveTap: Send {
    fn interval_id(&self) -> &str;
    fn start(&mut self, session: &str, frame: u64);
    fn audio(&mut self, source: usize, first: u64, stereo: &[f32]);
    fn gap(&mut self, source: usize, first: u64, end: u64);
    fn uncertain(&mut self, _source: usize, _observed_at: u64) {}
    fn speakers(&mut self, _enabled: bool, _frame: u64) {}
    fn recording(&mut self, _recording_id: &str, _start: u64, _end: u64) {}
    fn finish(&mut self, end: u64);
}

pub(super) enum Command {
    Attach(Box<dyn LiveTap>),
    StartRecording,
    StopRecording,
    StopLive,
    StopBoth,
    Speakers(bool),
}

pub(super) struct Activities {
    pub recording: bool,
    pub live: bool,
    pub events: VecDeque<(Instant, Command)>,
}

impl Activities {
    pub fn new(recording: bool, live: bool) -> Self {
        Self {
            recording,
            live,
            events: VecDeque::new(),
        }
    }

    pub fn change(&mut self, command: &Command) -> Result<bool, String> {
        match command {
            Command::Attach(_) if self.live => {
                return Err("A live transcript is already active".into());
            }
            Command::Attach(_) => self.live = true,
            Command::StartRecording if self.recording => return Ok(false),
            Command::StartRecording => self.recording = true,
            Command::StopRecording if !self.recording => return Ok(false),
            Command::StopRecording => self.recording = false,
            Command::StopLive if !self.live => return Ok(false),
            Command::StopLive => self.live = false,
            Command::StopBoth => {
                self.live = false;
                self.recording = false;
            }
            Command::Speakers(_) if !self.live => {
                return Err("Start transcription before changing speaker labels".into());
            }
            Command::Speakers(_) => {}
        }
        Ok(true)
    }
}

/// File creation uses create_new as its final collision check across processes.
pub(super) fn next_id() -> u128 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let previous = LAST
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
            Some(now.max(last.saturating_add(1)))
        })
        .unwrap();
    (u128::from(now.max(previous.saturating_add(1))) << 32) | u128::from(std::process::id())
}

pub(super) type RecordingReceipts = Arc<Mutex<VecDeque<Result<PathBuf, String>>>>;
pub(super) struct Consumers {
    pub closing: Vec<(PathBuf, JoinHandle<Result<(), String>>)>,
    pub receipts: RecordingReceipts,
    pub recording: Option<Recording>,
    pub live: Option<(u64, Box<dyn LiveTap>)>,
}

impl Consumers {
    pub fn attach(
        &mut self,
        mut tap: Box<dyn LiveTap>,
        config: &Config,
        session: &str,
        at: u64,
    ) -> Result<(), String> {
        if self.live.is_some() {
            return Err("A live transcript is already active".into());
        }
        if let Some(recording) = &mut self.recording {
            recording.enable_map(config, session, at)?;
        }
        tap.start(session, at);
        self.live = Some((at, tap));
        Ok(())
    }
    fn end_overlap(&mut self, at: u64) -> Result<(), String> {
        if let (Some(r), Some((first, tap))) = (&mut self.recording, &mut self.live) {
            let first = r.first.max(*first);
            if at > first {
                tap.recording(&r.id, first, at);
                if let Some(map) = &mut r.map {
                    map.overlap(tap.interval_id(), first, at)?;
                }
            }
        }
        Ok(())
    }
    pub fn stop_recording(
        &mut self,
        _config: &Config,
        at: u64,
        healthy: bool,
    ) -> Result<(), String> {
        let overlap = self.end_overlap(at);
        if let Some(r) = self.recording.take() {
            self.closing.push((
                r.first_path.clone(),
                r.finalize(at, healthy && overlap.is_ok()),
            ));
        }
        overlap
    }
    pub fn poll_finalizers(&mut self) -> Result<(), String> {
        let mut index = 0;
        let mut error = None;
        while index < self.closing.len() {
            if self.closing[index].1.is_finished() {
                let (path, worker) = self.closing.swap_remove(index);
                let result = worker
                    .join()
                    .unwrap_or_else(|_| Err("Recording writer panicked".into()))
                    .map_err(|e| format!("{e}: {}", path.display()))
                    .map(|()| path);
                if let Err(e) = &result {
                    error.get_or_insert(e.clone());
                }
                let mut receipts = self.receipts.lock().unwrap();
                if receipts.len() == 16 {
                    receipts.pop_front();
                }
                receipts.push_back(result);
            } else {
                index += 1;
            }
        }
        error.map_or(Ok(()), Err)
    }
    pub fn wait_finalizers(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut error = None;
        while !self.closing.is_empty() {
            if let Err(e) = self.poll_finalizers() {
                error.get_or_insert(e);
            }
            if self.closing.is_empty() {
                break;
            }
            if Instant::now() >= deadline {
                return Err("Recording writer finalization timed out".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        error.map_or(Ok(()), Err)
    }
    pub fn stop_live(&mut self, at: u64) -> Result<(), String> {
        let overlap = self.end_overlap(at);
        if let Some((_, mut tap)) = self.live.take() {
            tap.finish(at);
        }
        overlap
    }
    pub fn apply(
        &mut self,
        cmd: Command,
        config: &Config,
        shared: &Shared,
        at: u64,
    ) -> Result<(), String> {
        let opening_recording = matches!(cmd, Command::StartRecording);
        let opening_live = matches!(cmd, Command::Attach(_));
        let result = match cmd {
            Command::Attach(tap) => self.attach(tap, config, &shared.id, at),
            Command::StartRecording => (|| {
                if self.recording.is_some() {
                    return Ok(());
                }
                // A transcription-only session may have been permitted to select
                // overlapping sources. Validate the new writer's route before mixing.
                if let Some(error) = &shared.recording_route_error {
                    return Err(error.clone());
                }
                let mut r = Recording::open(config, at)?;
                if self.live.is_some() {
                    r.enable_map(config, &shared.id, at)?;
                }
                self.recording = Some(r);
                Ok(())
            })(),
            Command::StopRecording => self.stop_recording(config, at, true),
            Command::StopLive => self.stop_live(at),
            Command::StopBoth => {
                // Seal the live inlet first. Its worker can drain independently
                // while a codec flushes its own final pages.
                let live = self.stop_live(at);
                let audio = self.stop_recording(config, at, true);
                live.and(audio)
            }
            Command::Speakers(state) => {
                if let Some((_, tap)) = &mut self.live {
                    tap.speakers(state, at);
                }
                Ok(())
            }
        };
        if result.is_err() && (opening_recording || opening_live) {
            let mut ownership = shared.activities.lock().unwrap();
            if opening_recording {
                ownership.recording = false;
            }
            if opening_live {
                ownership.live = false;
            }
            if !ownership.recording && !ownership.live {
                shared.stop.store(true, Ordering::Release);
            }
        }
        result
    }
    pub fn send_audio(
        &mut self,
        sources: &mut [Source],
        frame: u64,
        valid: usize,
    ) -> Result<(), String> {
        if self.live.is_none() && self.recording.as_ref().is_none_or(|r| r.map.is_none()) {
            return Ok(());
        }
        let mut map_error = None;
        let end = frame + valid as u64;
        let block = frame / BLOCK * BLOCK;
        let silence = [0.0f32; BLOCK as usize * 2];
        for (index, source) in sources.iter_mut().enumerate() {
            let mut position = frame;
            let data = source.pending.get(&block);
            let unavailable = source.unavailable_from;
            let tap = &mut self.live;
            let mut map = self.recording.as_mut().and_then(|r| r.map.as_mut());
            let mut send = |first: u64, last: u64, gap: bool| -> Result<(), String> {
                if first >= last {
                    return Ok(());
                }
                let present_until = if gap {
                    first
                } else {
                    unavailable.unwrap_or(last).clamp(first, last)
                };
                if present_until > first
                    && let Some((_, t)) = tap
                {
                    let a = ((first - block) * 2) as usize;
                    let b = ((present_until - block) * 2) as usize;
                    let samples = data.map(|v| &v[a..b]).unwrap_or(&silence[..b - a]);
                    t.audio(index, first, samples);
                }
                if present_until < last {
                    if let Some((_, t)) = tap {
                        t.gap(index, present_until, last);
                    }
                    let cause = if gap { "packet_gap" } else { "source_ended" };
                    if let Some(m) = map.as_deref_mut()
                        && let Err(e) = m.source_gap(index, present_until, last, cause)
                    {
                        map_error.get_or_insert(e);
                    }
                }
                Ok(())
            };
            let mut uncertain = None;
            while let Some(&(first, last)) = source.timing.spans.front() {
                if first >= end {
                    break;
                }
                if last == first {
                    uncertain = Some(first.max(frame));
                    source.timing.spans.pop_front();
                    continue;
                }
                if last <= frame {
                    uncertain = Some(frame);
                    source.timing.spans.pop_front();
                    continue;
                }
                send(position, first.min(end).max(position), false)?;
                send(position.max(first), last.min(end), true)?;
                position = position.max(last.min(end));
                if last > end {
                    break;
                }
                source.timing.spans.pop_front();
            }
            send(position, end, false)?;
            if source.timing.overflow {
                uncertain = Some(frame);
                source.timing.overflow = false;
            }
            if let Some(observed_at) = uncertain {
                if let Some((_, t)) = tap {
                    t.uncertain(index, observed_at);
                }
                if let Some(m) = map {
                    if let Err(e) = m.uncertain(index, observed_at) {
                        map_error.get_or_insert(e);
                    }
                }
            }
        }
        map_error.map_or(Ok(()), Err)
    }
}

#[derive(Default)]
pub(super) struct TimingNotices {
    pub enabled: bool,
    spans: VecDeque<(u64, u64)>,
    uncertain_run: bool,
    overflow: bool,
}
impl TimingNotices {
    pub fn enable(&mut self, enabled: bool) {
        if self.enabled && !enabled {
            self.spans.clear();
            self.uncertain_run = false;
            self.overflow = false;
        }
        self.enabled = enabled;
    }
    pub fn observe(&mut self, frame: u64, event: capture::TimingEvent) {
        if !self.enabled {
            self.uncertain_run = false;
            return;
        }
        let was_uncertain = self.uncertain_run;
        self.uncertain_run = event == capture::TimingEvent::Uncertain;
        let notice = match event {
            capture::TimingEvent::KnownGap(count) => (frame.saturating_sub(count), frame),
            capture::TimingEvent::Uncertain if !was_uncertain => (frame, frame),
            _ => return,
        };
        if self.spans.len() >= 64 {
            self.overflow = true;
            return;
        }
        let i = self.spans.partition_point(|old| *old <= notice);
        self.spans.insert(i, notice);
    }
}
