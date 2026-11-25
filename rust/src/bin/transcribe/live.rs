//! Near-live transcription for an explicitly approved capture interval.
pub(crate) mod cache;
mod ingest;
mod recognize;
mod speakers;

use crate::{batch_queue, jobs, transport};
use audio_capture::live_wire::{Frame, MAX_LIVE_SOURCES, QUALIFIED_LIVE_GATE, read_frame};
use ingest::{Content, Piece, Source};
use recognize::{Recognizer, Request, Service};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, BufReader, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

const BACKLOG: usize = 24;
const DRAIN: Duration = Duration::from_secs(15);
const TICK: Duration = Duration::from_millis(25);

fn status(state: &str, message: &str, path: Option<&Path>, preview: Option<&str>) {
    let mut line = serde_json::json!({"state":state,"message":message});
    if let Some(p) = path {
        line["path"] = serde_json::json!(p.to_string_lossy())
    }
    if let Some(t) = preview {
        line["preview"] = serde_json::json!(t.chars().take(512).collect::<String>())
    }
    let mut out = io::stdout().lock();
    if serde_json::to_writer(&mut out, &line).is_ok() {
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
}

struct Options {
    endpoint: String,
    model: String,
    output: PathBuf,
    key_env: Option<String>,
    language: Option<String>,
    sources: Vec<String>,
    session: Option<String>,
    interval: Option<String>,
    cancel: Option<String>,
    gated: bool,
    timestamps: bool,
    provider: Option<transport::ProviderProfile>,
    speaker_config: Option<PathBuf>,
    evaluate_speakers: bool,
}
fn options(mut args: impl Iterator<Item = OsString>) -> Result<Options, String> {
    let mut o = Options {
        endpoint: String::new(),
        model: String::new(),
        output: PathBuf::new(),
        key_env: None,
        language: None,
        sources: Vec::new(),
        session: None,
        interval: None,
        cancel: None,
        gated: false,
        timestamps: false,
        provider: None,
        speaker_config: None,
        evaluate_speakers: false,
    };
    let (mut confirmed, mut evaluate_gate) = (false, false);
    while let Some(arg) = args.next() {
        let arg = arg.to_str().ok_or("Invalid live option")?;
        match arg {
            "--confirmed" => confirmed = true,
            "--evaluate-gate" => evaluate_gate = true,
            "--speech-gate" => o.gated = true,
            "--timestamps" => o.timestamps = true,
            "--provider-profile" => {
                let name = args
                    .next()
                    .and_then(|v| v.into_string().ok())
                    .ok_or("Missing provider profile")?;
                o.provider = Some(name.parse()?);
            }
            "--evaluate-speakers" => o.evaluate_speakers = true,
            "--output-dir" => o.output = args.next().ok_or("Missing output folder")?.into(),
            "--speaker-config" => {
                o.speaker_config = Some(args.next().ok_or("Missing speaker configuration")?.into())
            }
            "--base-url" | "--model" | "--source" | "--api-key-env" | "--language"
            | "--session-id" | "--interval-id" | "--cancel-event-id" => {
                let value = args
                    .next()
                    .and_then(|v| v.into_string().ok())
                    .ok_or_else(|| format!("Missing value for {arg}"))?;
                if value.is_empty() || value.chars().any(char::is_control) || value.len() > 4096 {
                    return Err(format!("Invalid value for {arg}"));
                }
                match arg {
                    "--base-url" => o.endpoint = value,
                    "--model" => o.model = value,
                    "--api-key-env" => o.key_env = Some(value),
                    "--language" => o.language = Some(value),
                    "--session-id" => o.session = Some(value),
                    "--interval-id" => o.interval = Some(value),
                    "--cancel-event-id" => o.cancel = Some(value),
                    "--source" => {
                        if value.len() > 256
                            || o.sources.len() >= MAX_LIVE_SOURCES
                            || o.sources.contains(&value)
                        {
                            return Err(
                                "Live sources must have distinct labels (up to 16 sources)".into(),
                            );
                        }
                        o.sources.push(value)
                    }
                    _ => unreachable!(),
                }
            }
            _ => return Err(format!("Unknown live option {arg}")),
        }
    }
    if !confirmed
        || o.sources.is_empty()
        || o.output.as_os_str().is_empty()
        || o.model.is_empty()
        || o.endpoint.is_empty()
    {
        return Err(
            "Live transcription requires confirmed sources, endpoint, model, and output folder"
                .into(),
        );
    }
    if o.timestamps && o.provider.is_some() {
        return Err("Choose timestamps or a provider profile".into());
    }
    if (evaluate_gate && !o.gated) || (o.gated && !QUALIFIED_LIVE_GATE && !evaluate_gate) {
        return Err("Live speech gating requires the explicit evaluation profile".into());
    }
    if o.evaluate_speakers && o.speaker_config.is_none() {
        return Err("Speaker evaluation requires a local speaker configuration".into());
    }
    Ok(o)
}

pub(crate) fn run(args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let result = run_interval(args);
    if let Err(e) = &result {
        status("error", e, None, None)
    }
    result
}

struct CancelEvent(OwnedHandle);
impl CancelEvent {
    fn open(id: &str) -> Result<Self, String> {
        if id.len() > 128
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        {
            return Err("Invalid cancel event ID".into());
        }
        use windows_sys::Win32::System::Threading::{OpenEventW, SYNCHRONIZATION_SYNCHRONIZE};
        let name: Vec<_> = format!(r"Local\AudioCapture.Live.Cancel.{id}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let h = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
        if h.is_null() {
            Err(format!(
                "Cannot open live cancel event: {}",
                io::Error::last_os_error()
            ))
        } else {
            Ok(Self(unsafe { OwnedHandle::from_raw_handle(h) }))
        }
    }
    fn signaled(&self) -> bool {
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(self.0.as_raw_handle(), 0)
                == windows_sys::Win32::Foundation::WAIT_OBJECT_0
        }
    }
}

fn run_interval(args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let opt = options(args)?;
    let dest = transport::endpoint(&opt.endpoint, false)?;
    let canonical = transport::canonical_key(&dest);
    let cancel = opt
        .cancel
        .as_deref()
        .map(CancelEvent::open)
        .transpose()?
        .map(Arc::new);
    let key = opt
        .key_env
        .as_ref()
        .map(|name| {
            std::env::var(name).map_err(|_| "Configured bearer token is unavailable".to_string())
        })
        .transpose()?;
    if key
        .as_ref()
        .is_some_and(|s| s.is_empty() || !s.bytes().all(|c| (0x21..=0x7e).contains(&c)))
    {
        return Err("Configured bearer token is invalid".into());
    }
    let Some(_lease) = batch_queue::reserve_live(
        &canonical,
        || status("waiting", "Waiting for batch request", None, None),
        || cancel.as_deref().is_some_and(CancelEvent::signaled),
    )?
    else {
        status(
            "cancelled",
            "Live start cancelled before admission",
            None,
            None,
        );
        return Ok(());
    };
    status("ready", "Ready for confirmed sources", None, None);
    let mut input = BufReader::new(io::stdin());
    let Some(first_frame) = read_frame(&mut input).map_err(|e| e.to_string())? else {
        status("cancelled", "Live start cancelled", None, None);
        return Ok(());
    };
    let Frame::Start {
        session,
        interval,
        first,
        sources,
    } = first_frame
    else {
        return Err("Expected live start metadata".into());
    };
    if sources != opt.sources
        || opt.session.as_ref().is_some_and(|s| s != &session)
        || opt.interval.as_ref().is_some_and(|s| s != &interval)
        || opt.cancel.as_ref().is_some_and(|s| s != &interval)
    {
        return Err("Live stream differs from confirmed scope".into());
    }
    let file =
        cache::LiveCache::create_with_sources(&session, &interval, &opt.output, first, &sources)?;
    status(
        "active",
        "Transcribing selected sources",
        Some(&file.partial_path()),
        None,
    );
    let setting_key = jobs::digest(
        serde_json::to_vec(&(
            1,
            &session,
            &interval,
            &canonical,
            &opt.model,
            &opt.language,
            opt.gated,
            opt.timestamps,
            opt.provider,
            if opt.gated {
                "pcm48k-mono-4s-zero-core-pre160-post240-v1"
            } else {
                "pcm48k-mono-4s"
            },
        ))
        .map_err(|e| e.to_string())?
        .as_slice(),
    )?;
    let service = Service::new(Arc::new(Recognizer {
        endpoint: dest,
        model: opt.model,
        key,
        language: opt.language,
        timestamps: opt.timestamps,
        provider: opt.provider,
        cancel: cancel.clone(),
    }))?;
    let (events, rx) = mpsc::sync_channel(8);
    std::thread::Builder::new()
        .name("live-input".into())
        .spawn(move || {
            loop {
                let message = read_frame(&mut input).map_err(|e| e.to_string());
                let finished =
                    !matches!(&message,Ok(Some(frame)) if !matches!(frame,Frame::End{..}));
                if events.send(message).is_err() || finished {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    let mut engine = Engine::new(
        sources,
        first,
        opt.gated,
        file,
        service,
        setting_key,
        opt.speaker_config,
        opt.evaluate_speakers,
        opt.provider.is_some(),
    );
    let mut drain: Option<(u64, Instant)> = None;
    let mut extra = Vec::new();
    loop {
        let cancelled = cancel.as_deref().is_some_and(CancelEvent::signaled);
        let expired = drain.is_some_and(|(_, at)| Instant::now() >= at);
        if cancelled || expired {
            engine.abandon(if cancelled {
                "cancelled during finalization"
            } else {
                "live drain deadline expired"
            })?;
            if let Some((end, _)) = drain {
                return engine.publish(end);
            }
            // A cancellation event can beat the input pipe's End. Continue consuming only
            // coverage until the controller's cutoff arrives; never dispatch another upload.
        }
        let activity = if drain.is_some() { "saving" } else { "active" };
        engine.receive_reply(activity)?;
        engine.poll_speakers(activity)?;
        engine.dispatch(drain.map(|(_, at)| at))?;
        if let Some((end, _)) = drain {
            if engine.finished() {
                return engine.publish(end);
            }
            std::thread::sleep(TICK);
            continue;
        }
        let message = match rx.recv_timeout(TICK) {
            Ok(m) => m,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(None),
        };
        let seal = match message {
            Ok(Some(Frame::Audio { source, first, pcm })) => {
                let idx = source as usize;
                let track = engine.tracks.get(idx).ok_or("Unconfirmed live source")?;
                let skip = track.next.saturating_sub(first).min(pcm.len() as u64) as usize;
                if !engine.no_more_requests && skip < pcm.len() {
                    engine.feed_speaker(idx, first + skip as u64, &pcm[skip..]);
                }
                engine.tracks[idx].audio(first, &pcm, &mut extra)?;
                None
            }
            Ok(Some(Frame::Gap {
                source,
                first,
                end,
                reason,
            })) => {
                let idx = source as usize;
                engine
                    .tracks
                    .get_mut(idx)
                    .ok_or("Unconfirmed live gap source")?
                    .gap(first, end, reason.as_str(), &mut extra);
                None
            }
            Ok(Some(Frame::Timing { source, frame })) => {
                let label = engine
                    .labels
                    .get(source as usize)
                    .ok_or("Unconfirmed live source")?;
                engine.file.timing(label, frame)?;
                None
            }
            Ok(Some(Frame::Speakers { enabled, frame })) => {
                engine.toggle_speakers(enabled, frame);
                None
            }
            Ok(Some(Frame::Recording { id, start, end })) => {
                engine.file.recording(&id, start, end)?;
                None
            }
            Ok(Some(Frame::End { end })) => Some(end),
            Ok(Some(Frame::Start { .. })) => return Err("Duplicate live start".into()),
            Ok(None) | Err(_) => {
                engine.incomplete = true;
                status(
                    "saving",
                    "Input ended before a verified cutoff",
                    Some(&engine.file.partial_path()),
                    None,
                );
                Some(engine.tracks.iter().map(|s| s.next).max().unwrap_or(first))
            }
        };
        if let Some(end) = seal {
            if end < first || engine.tracks.iter().any(|s| s.next > end) {
                return Err("Invalid live cutoff before received audio".into());
            }
            for s in &mut engine.tracks {
                s.gap(s.next, end, "missing source tail", &mut extra);
                s.seal(&mut extra);
            }
            if let Some(speaker) = &mut engine.speakers {
                if let Err(e) = speaker.seal() {
                    status("saving", &e, None, None);
                }
            }
            status(
                "saving",
                "Finalizing transcript",
                Some(&engine.file.partial_path()),
                None,
            );
            drain = Some((end, Instant::now() + DRAIN));
        }
        for p in extra.drain(..) {
            engine.enqueue(p);
        }
    }
}

struct Outstanding {
    id: String,
    source: usize,
    first: u64,
    end: u64,
    submitted: Instant,
    deadline: Instant,
    marked_unknown: bool,
}
struct Engine {
    labels: Vec<String>,
    tracks: Vec<Source>,
    file: cache::LiveCache,
    service: Service,
    key: String,
    queue: VecDeque<Piece>,
    overflow: Vec<Option<(u64, u64)>>,
    active: Option<Outstanding>,
    incomplete: bool,
    no_more_requests: bool,
    speakers: Option<speakers::LiveSpeakers>,
    speaker_config: Option<PathBuf>,
    evaluation: bool,
    epoch: u64,
    speaker_start: u64,
    samplers: Vec<Downsample>,
    provider_labels: bool,
}
impl Engine {
    #[allow(clippy::too_many_arguments)]
    fn new(
        labels: Vec<String>,
        first: u64,
        gate: bool,
        file: cache::LiveCache,
        service: Service,
        key: String,
        speaker_config: Option<PathBuf>,
        evaluation: bool,
        provider_labels: bool,
    ) -> Self {
        let n = labels.len();
        Self {
            labels,
            tracks: (0..n).map(|i| Source::new(i, first, gate)).collect(),
            file,
            service,
            key,
            queue: VecDeque::new(),
            overflow: vec![None; n],
            active: None,
            incomplete: false,
            no_more_requests: false,
            speakers: None,
            speaker_config,
            evaluation,
            epoch: 0,
            speaker_start: first,
            samplers: (0..n).map(|_| Downsample::default()).collect(),
            provider_labels,
        }
    }

    fn enqueue(&mut self, p: Piece) {
        let i = p.source;
        if self.queue.len() >= BACKLOG || self.overflow[i].is_some() {
            match &mut self.overflow[i] {
                Some((_, e)) => *e = p.end,
                slot => {
                    *slot = Some((p.first, p.end));
                    status(
                        "active",
                        "Recognition backlog full; missing audio will be marked",
                        None,
                        None,
                    );
                }
            }
            self.incomplete = true;
        } else {
            self.queue.push_back(p)
        }
    }

    fn refill(&mut self) {
        for (source, mark) in self.overflow.iter_mut().enumerate() {
            if self.queue.len() >= BACKLOG {
                break;
            }
            if let Some((first, end)) = mark.take() {
                self.queue.push_back(Piece {
                    source,
                    first,
                    end,
                    content: Content::Gap("recognition backlog full"),
                });
            }
        }
    }

    fn dispatch(&mut self, drain: Option<Instant>) -> Result<(), String> {
        self.refill();
        while self.active.is_none() {
            let Some(p) = self.queue.pop_front() else {
                break;
            };
            self.refill();
            let label = &self.labels[p.source];
            let event_id = format!("range-{}-{}-{}", p.source, p.first, p.end);
            match p.content {
                Content::Quiet => self.file.skipped(&event_id, label, p.first, p.end)?,
                Content::Gap(reason) => {
                    self.incomplete = true;
                    self.file.gap(&event_id, label, p.first, p.end, reason)?;
                }
                Content::Audio(pcm) => {
                    if self.no_more_requests || drain.is_some_and(|d| Instant::now() >= d) {
                        self.incomplete = true;
                        self.file.gap(
                            &event_id,
                            label,
                            p.first,
                            p.end,
                            "recognition was stopped before submission",
                        )?;
                        continue;
                    }
                    let wav = recognize::wav(pcm);
                    let content_hash = jobs::digest(wav.as_slice())?;
                    let identity = jobs::digest(
                        serde_json::to_vec(&(
                            &self.key,
                            p.source,
                            label,
                            p.first,
                            p.end,
                            content_hash,
                        ))
                        .map_err(|e| e.to_string())?
                        .as_slice(),
                    )?;
                    self.file.request(&identity, label, p.first, p.end)?;
                    let submitted = Instant::now();
                    let deadline = submitted + recognize::REQUEST_TIMEOUT;
                    let deadline = drain.map_or(deadline, |end| end.min(deadline));
                    self.service
                        .send
                        .send(Request { wav, deadline })
                        .map_err(|_| "Recognition service ended")?;
                    self.active = Some(Outstanding {
                        id: identity,
                        source: p.source,
                        first: p.first,
                        end: p.end,
                        submitted,
                        deadline,
                        marked_unknown: false,
                    });
                }
            }
        }
        Ok(())
    }

    fn receive_reply(&mut self, state: &str) -> Result<(), String> {
        let Some(job) = self.active.as_mut() else {
            return Ok(());
        };
        let response = match self.service.recv.try_recv() {
            Ok(reply) => reply.result,
            Err(mpsc::TryRecvError::Empty) => {
                if Instant::now() >= job.deadline && !job.marked_unknown {
                    self.file.gap(
                        &job.id,
                        &self.labels[job.source],
                        job.first,
                        job.end,
                        "request timed out; remote outcome unknown",
                    )?;
                    self.incomplete = true;
                    job.marked_unknown = true;
                    status(
                        state,
                        "Request timed out; remote outcome unknown",
                        None,
                        None,
                    );
                }
                return Ok(());
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Recognition service ended; remote outcome unknown".into())
            }
        };
        let job = self.active.take().unwrap();
        if job.marked_unknown {
            return Ok(());
        }
        let label = &self.labels[job.source];
        match response {
            Ok((text, raw)) => {
                let segments = raw
                    .as_ref()
                    .and_then(|r| timed(r, &text, job.first, job.end, self.provider_labels));
                if let Some(spans) = segments {
                    self.file
                        .speech_timed(&job.id, label, job.first, job.end, &text, spans)?;
                } else {
                    self.file
                        .speech(&job.id, label, job.first, job.end, None, &text)?;
                }
                let lag = self.tracks[job.source].next.saturating_sub(job.end) as f64
                    / ingest::RATE as f64;
                let response_seconds = job.submitted.elapsed().as_secs_f64();
                status(
                    state,
                    &format!(
                        "{label}: text {lag:.1}s behind input. Request {response_seconds:.1}s."
                    ),
                    Some(&self.file.partial_path()),
                    self.file.last_preview(),
                );
            }
            Err(e) => {
                self.incomplete = true;
                self.file.gap(&job.id, label, job.first, job.end, &e)?;
                status(state, &e, Some(&self.file.partial_path()), None);
            }
        }
        Ok(())
    }

    fn toggle_speakers(&mut self, enabled: bool, frame: u64) {
        if !enabled {
            self.speakers = None;
            self.samplers
                .iter_mut()
                .for_each(|s| *s = Downsample::default());
            status("active", "Local speaker labels off", None, None);
            return;
        }
        if self.no_more_requests || self.speakers.is_some() {
            return;
        }
        let Some(path) = self.speaker_config.as_deref() else {
            status(
                "active",
                "Live speaker labels unavailable: no local configuration",
                None,
                None,
            );
            return;
        };
        self.epoch = self.epoch.saturating_add(1).max(frame);
        let adapter = if self.evaluation {
            speakers::LiveSpeakers::start_evaluation(path, self.epoch)
        } else {
            speakers::LiveSpeakers::start(path, self.epoch)
        };
        match adapter {
            Ok(adapter) => {
                self.speakers = Some(adapter);
                self.speaker_start = frame;
                status("active", "Local speaker labels on", None, None);
            }
            Err(e) => status(
                "active",
                &format!("Local speaker labels unavailable: {e}"),
                None,
                None,
            ),
        }
    }

    fn feed_speaker(&mut self, idx: usize, first: u64, pcm: &[i16]) {
        if let Some(adapter) = &mut self.speakers {
            let trim = self
                .speaker_start
                .saturating_sub(first)
                .min(pcm.len() as u64) as usize;
            if trim == pcm.len() {
                return;
            }
            if let Some((offset, audio)) =
                self.samplers[idx].feed(first + trim as u64, &pcm[trim..])
            {
                if let Err(e) = adapter.submit(idx as u32, offset, &audio) {
                    status("active", &e, None, None);
                }
            }
        }
    }

    fn poll_speakers(&mut self, state: &str) -> Result<(), String> {
        if let Some(adapter) = &mut self.speakers {
            for update in adapter.poll() {
                let src = update.source as usize;
                if src >= self.labels.len() {
                    continue;
                }
                if update.first_frame == update.end_frame
                    && let Some(message) = &update.diagnostic
                {
                    status(state, message, Some(&self.file.partial_path()), None);
                    continue;
                }
                self.file.speaker_revision(
                    &self.labels[src],
                    update.first_frame,
                    update.end_frame,
                    update.revision,
                    &update,
                )?;
                status(
                    state,
                    update
                        .diagnostic
                        .as_deref()
                        .unwrap_or("Provisional local speaker update"),
                    Some(&self.file.partial_path()),
                    self.file.last_preview(),
                );
            }
            if adapter.failed() {
                self.speakers = None;
            }
        }
        Ok(())
    }

    fn abandon(&mut self, reason: &str) -> Result<(), String> {
        self.no_more_requests = true;
        self.speakers = None;
        if let Some(job) = self.active.take() {
            if !job.marked_unknown {
                self.file.gap(
                    &job.id,
                    &self.labels[job.source],
                    job.first,
                    job.end,
                    &format!("{reason}; remote outcome unknown"),
                )?;
            }
            self.incomplete = true;
        }
        self.dispatch(None)
    }

    fn finished(&self) -> bool {
        self.active.is_none()
            && self.queue.is_empty()
            && self.overflow.iter().all(Option::is_none)
            && self.speakers.as_ref().is_none_or(|s| s.pending() == 0)
    }
    fn publish(&mut self, end: u64) -> Result<(), String> {
        let path = self.file.finish(end, self.incomplete).map_err(|e| {
            format!(
                "{e}. Recoverable text: {}",
                self.file.partial_path().display()
            )
        })?;
        status(
            if self.incomplete {
                "incomplete"
            } else {
                "complete"
            },
            if self.incomplete {
                "Transcript saved with incomplete coverage"
            } else {
                "Transcript saved"
            },
            Some(&path),
            None,
        );
        Ok(())
    }
}

fn timed(
    raw: &serde_json::Value,
    text: &str,
    first: u64,
    end: u64,
    provider_labels: bool,
) -> Option<Vec<cache::TimedSpeech>> {
    let segments = raw.get("segments")?.as_array()?;
    if segments.is_empty() || segments.len() > 4096 {
        return None;
    }
    let mut tail = first;
    use crate::attribution::inputs::match_texts;
    let ranges = match_texts(text, segments, "text", false).or_else(|| {
        if provider_labels {
            match_texts(text, segments, "text", true)
        } else {
            None
        }
    })?;
    let mut result = Vec::new();
    for (segment, [begin, _, finish]) in segments.iter().zip(ranges) {
        let time = |name: &str| -> Option<u64> {
            let sec = segment.get(name)?.as_f64()?;
            if !sec.is_finite() || sec < 0. || sec > (end - first) as f64 / ingest::RATE as f64 {
                return None;
            }
            first.checked_add((sec * ingest::RATE as f64).round() as u64)
        };
        let start = time("start")?;
        let stop = time("end")?;
        if start < tail || stop <= start || stop > end {
            return None;
        }
        tail = stop;
        let speaker = if provider_labels {
            segment
                .get("speaker")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|id| !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control))
                .map(|id| format!("Provider chunk {first}: {id}"))
        } else {
            None
        };
        result.push(cache::TimedSpeech {
            start_frame: start,
            end_frame: stop,
            text: text.get(begin..finish)?.into(),
            speaker,
        });
    }
    Some(result)
}

/// Average consecutive triples, carrying partial groups across input packets. Speaker times
/// stay on the original timeline even for short packets and source discontinuities.
#[derive(Default)]
struct Downsample {
    next: Option<u64>,
    first: u64,
    sum: i32,
    count: u8,
}
impl Downsample {
    fn feed(&mut self, first: u64, pcm: &[i16]) -> Option<(u64, Vec<i16>)> {
        if self.next != Some(first) {
            self.first = first;
            self.count = 0;
            self.sum = 0;
        }
        let offset = self.first;
        let mut out = Vec::with_capacity(pcm.len().div_ceil(3));
        for &v in pcm {
            self.sum += i32::from(v);
            self.count += 1;
            if self.count == 3 {
                out.push((self.sum / 3) as i16);
                self.first += 3;
                self.count = 0;
                self.sum = 0;
            }
        }
        self.next = Some(first + pcm.len() as u64);
        (!out.is_empty()).then_some((offset, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gate_preserves_approved_endpoint_model_and_source_layout() {
        for labels in [vec!["Discord"], vec!["Discord", "USB microphone"]] {
            let mut args: Vec<OsString> = [
                "--confirmed",
                "--base-url",
                "http://localhost:8024/custom/v1",
                "--model",
                "speech-custom",
                "--output-dir",
                "transcripts",
            ]
            .into_iter()
            .map(Into::into)
            .collect();
            for label in &labels {
                args.extend([OsString::from("--source"), OsString::from(label)]);
            }
            let default = options(args.clone().into_iter()).unwrap();
            assert!(!default.gated);
            args.push("--speech-gate".into());
            assert_eq!(
                options(args.clone().into_iter()).is_ok(),
                QUALIFIED_LIVE_GATE
            );
            args.push("--evaluate-gate".into());
            let gated = options(args.into_iter()).unwrap();
            assert!(gated.gated);
            assert_eq!(gated.endpoint, default.endpoint);
            assert_eq!(gated.model, default.model);
            assert_eq!(gated.sources, labels);
        }
    }
    #[test]
    fn decimator_carries_packet_phase_and_resets_after_loss() {
        let mut s = Downsample::default();
        assert!(s.feed(501, &[1, 2]).is_none());
        assert_eq!(s.feed(503, &[3, 6, 9, 12, 15]), Some((501, vec![2, 9])));
        assert_eq!(s.feed(800, &[9, 12, 15]), Some((800, vec![12])));
    }
    #[test]
    fn invalid_or_unmatched_timing_keeps_raw_text_available() {
        let m = serde_json::json!({"segments":[{"start":0,"end":0.5,"text":" One"},{"start":0.5,"end":1,"text":" one"}]});
        let spans = timed(&m, " One one", 48000, 96000, false).unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1].start_frame, 72000);
        let preserved = timed(&m, "One  one\n", 48000, 96000, false).unwrap();
        assert_eq!(
            preserved
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>(),
            "One  one\n"
        );
        assert!(timed(&m, "One two", 48000, 96000, false).is_none());
        let response = serde_json::json!({"segments":[{"start":0,"end":1,"text":" Hello world.","speaker":"A"}]});
        let spans = timed(&response, "Hello world.", 0, 48000, true).unwrap();
        assert_eq!(spans[0].text, "Hello world.");
        assert_eq!(spans[0].speaker.as_deref(), Some("Provider chunk 0: A"));
        let prefixed = timed(&response, "A: Hello world.", 0, 48000, true).unwrap();
        assert_eq!(prefixed[0].text, "A: Hello world.");
    }
}
