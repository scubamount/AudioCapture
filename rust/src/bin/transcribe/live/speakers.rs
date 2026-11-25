//! Bounded local speaker analysis for one explicitly enabled live interval.
//! The recorder never links this module. Only the worker owns the model job.
use crate::attribution::adapter;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::io::OwnedHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};

const CORE: usize = 4 * 16_000;
const LOOKBACK_FRAMES: u64 = 24 * 48_000;
const MAX_PENDING: usize = 8;
const MAX_SOURCES: u32 = 16;
const MAX_REPLY: usize = 128 * 1024;
const PROTOCOL: &str = "live-window-v1";

struct QualifiedProfile {
    bridge_sha256: &'static str,
    backend: &'static str,
    segmentation_sha256: &'static str,
    embedding_sha256: &'static str,
    threads: u8,
    threshold: f32,
}

// Fill this only from an approved live qualification receipt.
const QUALIFIED_PROFILES: &[QualifiedProfile] = &[];
const NO_PROFILE: &str =
    "Live speaker labels have no qualified release profile. Use offline refinement on saved audio.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    protocol: String,
    python: PathBuf,
    bridge: PathBuf,
    segmentation: PathBuf,
    embedding: PathBuf,
    #[serde(default = "default_threads")]
    threads: u8,
    #[serde(default = "default_threshold")]
    threshold: f32,
}

fn default_threads() -> u8 {
    2
}

fn default_threshold() -> f32 {
    0.5
}

impl Config {
    fn read(path: &Path) -> Result<Self, String> {
        let check_local = |p: &Path| {
            crate::attribution::export::local_destination(p).map_err(|_| {
                "Choose a local drive for the speaker configuration and model files.".to_owned()
            })
        };
        check_local(path)?;
        let mut bytes = Vec::new();
        File::open(path)
            .and_then(|f| f.take(65_537).read_to_end(&mut bytes))
            .map_err(|e| format!("Cannot read live speaker configuration: {e}"))?;
        if bytes.len() > 65_536 {
            return Err("Live speaker configuration exceeds 64 KiB.".into());
        }
        let config: Self = serde_json::from_slice(&bytes)
            .map_err(|_| "Live labels require a live-window-v1 configuration. File adapters support offline refinement.".to_owned())?;
        if config.protocol != PROTOCOL
            || !(1..=32).contains(&config.threads)
            || !(0.0 < config.threshold && config.threshold < 2.0)
        {
            return Err("Invalid live speaker configuration.".into());
        }
        for path in [
            &config.python,
            &config.bridge,
            &config.segmentation,
            &config.embedding,
        ] {
            check_local(path)?;
            if !path.is_absolute() || !path.is_file() {
                return Err("The live speaker runtime, bridge and models require existing absolute file paths.".into());
            }
        }
        Ok(config)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerTurn {
    pub start: u64,
    pub end: u64,
    pub speaker: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpeakerUpdate {
    pub source: u32,
    pub first_frame: u64,
    pub end_frame: u64,
    pub turns: Vec<SpeakerTurn>,
    pub diagnostic: Option<String>,
    pub epoch: u64,
    pub revision: u64,
    pub backend: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Range {
    source: u32,
    first: u64,
    end: u64,
}

impl Range {
    fn missing(self, epoch: u64, why: &str) -> SpeakerUpdate {
        SpeakerUpdate {
            source: self.source,
            first_frame: self.first,
            end_frame: self.end,
            turns: Vec::new(),
            diagnostic: Some(why.into()),
            epoch,
            revision: 0,
            backend: None,
            model: None,
        }
    }
}

struct Window {
    range: Range,
    pcm: Vec<i16>,
}

struct Core {
    first: u64,
    pcm: Vec<i16>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    protocol: String,
    ready: bool,
    backend: String,
    segmentation_sha256: String,
    embedding_sha256: String,
    threads: u8,
    threshold: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    source: u32,
    first_frame: u64,
    end_frame: u64,
    replace_from: u64,
    turns: Vec<SpeakerTurn>,
}

enum Event {
    Result(SpeakerUpdate),
    Failed(String),
}

enum Watch {
    Deadline(Duration),
    Idle,
    Stop,
}

fn watchdog(job: OwnedHandle, control: Receiver<Watch>) {
    let mut limit = Some(Duration::from_secs(30));
    loop {
        let next = match limit {
            Some(limit) => control.recv_timeout(limit).ok(),
            None => control.recv().ok(),
        };
        match next {
            Some(Watch::Deadline(value)) => limit = Some(value),
            Some(Watch::Idle) => limit = None,
            Some(Watch::Stop) | None => break,
        }
    }
    // Also terminates descendants holding a pipe after their parent exits.
    drop(job);
}

fn line<T: serde::de::DeserializeOwned>(reader: &mut impl BufRead) -> Result<T, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_REPLY as u64 + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|e| format!("Cannot read a live speaker reply: {e}"))?;
    if bytes.len() > MAX_REPLY || bytes.last() != Some(&b'\n') {
        return Err("Live speaker pipe closed, timed out, or exceeded its reply limit.".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid live speaker reply.".into())
}

fn exchange(
    mut input: impl Write,
    output: impl Read,
    windows: Receiver<Window>,
    events: &SyncSender<Event>,
    watch: &Sender<Watch>,
    epoch: u64,
    qualified: Option<&QualifiedProfile>,
) -> Result<(), String> {
    let mut reader = BufReader::new(output);
    let ready: Ready = line(&mut reader)?;
    let hash = |s: &str| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit());
    if !ready.ready
        || ready.protocol != PROTOCOL
        || ready.backend.is_empty()
        || ready.backend.len() > 128
        || ready.backend.chars().any(char::is_control)
        || !hash(&ready.segmentation_sha256)
        || !hash(&ready.embedding_sha256)
        || !(1..=32).contains(&ready.threads)
        || !(ready.threshold > 0.0 && ready.threshold < 2.0)
    {
        return Err("Live speaker adapter did not identify a compatible local model.".into());
    }
    if qualified.is_some_and(|p| {
        p.backend != ready.backend
            || p.segmentation_sha256 != ready.segmentation_sha256
            || p.embedding_sha256 != ready.embedding_sha256
            || p.threads != ready.threads
            || p.threshold != ready.threshold
    }) {
        return Err(NO_PROFILE.into());
    }
    let model = format!(
        "segmentation={};embedding={};threshold={};threads={}",
        ready.segmentation_sha256, ready.embedding_sha256, ready.threshold, ready.threads
    );
    let mut wire = Vec::with_capacity(CORE * 2 + 128);
    let mut previous: BTreeMap<u32, (u64, u64)> = BTreeMap::new();
    let mut revision = 0;
    let _ = watch.send(Watch::Idle);
    for window in windows {
        let _ = watch.send(Watch::Deadline(Duration::from_secs(12)));
        let Range { source, first, end } = window.range;
        wire.clear();
        writeln!(
            &mut wire,
            "{{\"source\":{source},\"first_frame\":{first},\"samples\":{}}}",
            window.pcm.len()
        )
        .map_err(|e| e.to_string())?;
        for sample in &window.pcm {
            wire.extend_from_slice(&sample.to_le_bytes());
        }
        input
            .write_all(&wire)
            .and_then(|_| input.flush())
            .map_err(|e| format!("Cannot send local speaker audio: {e}"))?;
        let mut reply: Reply = line(&mut reader)?;
        let allowed = previous
            .get(&source)
            .filter(|(_, last)| *last == first)
            .map_or(first, |(begin, _)| {
                (*begin).max(first.saturating_sub(LOOKBACK_FRAMES))
            });
        if reply.source != source
            || reply.first_frame != first
            || reply.end_frame != end
            || !(allowed..=first).contains(&reply.replace_from)
            || reply.turns.len() > 512
        {
            return Err("Local speaker reply identified the wrong source or frame range.".into());
        }
        for turn in &mut reply.turns {
            if turn.start < reply.replace_from
                || turn.start >= turn.end
                || turn.end > end
                || turn.speaker.len() > 20
                || turn.speaker.parse::<u64>().is_err()
                || !turn.speaker.bytes().all(|b| b.is_ascii_digit())
            {
                return Err("Invalid provisional speaker interval.".into());
            }
            turn.speaker = format!("live-{epoch}-{source}-{}", turn.speaker);
        }
        previous.insert(source, (allowed, end));
        revision += 1;
        let _ = watch.send(Watch::Idle);
        let update = SpeakerUpdate {
            source,
            first_frame: reply.replace_from,
            end_frame: end,
            turns: reply.turns,
            diagnostic: None,
            epoch,
            revision,
            backend: Some(ready.backend.clone()),
            model: Some(model.clone()),
        };
        if events.send(Event::Result(update)).is_err() {
            break;
        }
    }
    Ok(())
}

/// A dropped handle seals admission and kills the job without waiting for model inference.
pub struct LiveSpeakers {
    epoch: u64,
    last_frame: [Option<u64>; MAX_SOURCES as usize],
    cores: BTreeMap<u32, Core>,
    pending: VecDeque<Range>,
    missed: [Option<Range>; MAX_SOURCES as usize],
    input: Option<SyncSender<Window>>,
    events: Option<Receiver<Event>>,
    control: Sender<Watch>,
    threads: Vec<JoinHandle<()>>,
    sealed: bool,
    failed: bool,
}

impl LiveSpeakers {
    pub fn start(config: &Path, epoch: u64) -> Result<Self, String> {
        if !audio_capture::live_wire::QUALIFIED_LIVE_SPEAKERS {
            return Err(NO_PROFILE.into());
        }
        let config = Config::read(config)?;
        let bridge = crate::jobs::digest(File::open(&config.bridge).map_err(|e| e.to_string())?)?;
        let profile = QUALIFIED_PROFILES
            .iter()
            .find(|p| {
                p.bridge_sha256 == bridge
                    && p.threads == config.threads
                    && p.threshold == config.threshold
            })
            .ok_or(NO_PROFILE)?;
        Self::launch(config, epoch, Some(profile))
    }

    pub fn start_evaluation(config: &Path, epoch: u64) -> Result<Self, String> {
        Self::launch(Config::read(config)?, epoch, None)
    }

    fn launch(
        config: Config,
        epoch: u64,
        qualified: Option<&'static QualifiedProfile>,
    ) -> Result<Self, String> {
        let job = adapter::job()?;
        let mut child = Command::new(config.python)
            .args(["-I", "-B"])
            .arg(config.bridge)
            .arg("--live-windows")
            .arg("--segmentation")
            .arg(config.segmentation)
            .arg("--embedding")
            .arg(config.embedding)
            .arg("--threads")
            .arg(config.threads.to_string())
            .arg("--threshold")
            .arg(config.threshold.to_string())
            .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Cannot start local speaker runtime: {e}"))?;
        if let Err(error) = adapter::start_in_job(&child, &job) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Self::connect(child, job, epoch, qualified)
    }

    fn connect(
        mut child: Child,
        job: OwnedHandle,
        epoch: u64,
        qualified: Option<&'static QualifiedProfile>,
    ) -> Result<Self, String> {
        let stdin = child.stdin.take().ok_or("Missing speaker input pipe")?;
        let stdout = child.stdout.take().ok_or("Missing speaker output pipe")?;
        let (input, windows) = mpsc::sync_channel(MAX_PENDING);
        let (send, events) = mpsc::sync_channel(MAX_PENDING + 1);
        let (control, watch_events) = mpsc::channel();
        let guard = thread::Builder::new()
            .name("speaker-deadline".into())
            .spawn(move || watchdog(job, watch_events))
            .map_err(|e| e.to_string())?;
        let watch = control.clone();
        let io = thread::Builder::new()
            .name("local-speaker-pipe".into())
            .spawn(move || {
                let result = exchange(stdin, stdout, windows, &send, &watch, epoch, qualified);
                let _ = watch.send(Watch::Stop);
                let _ = child.wait();
                if let Err(error) = result {
                    let _ = send.send(Event::Failed(error));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            epoch,
            last_frame: [None; MAX_SOURCES as usize],
            cores: BTreeMap::new(),
            pending: VecDeque::new(),
            missed: [None; MAX_SOURCES as usize],
            input: Some(input),
            events: Some(events),
            control,
            threads: vec![guard, io],
            sealed: false,
            failed: false,
        })
    }

    fn enqueue(&mut self, source: u32, core: Core) -> Result<(), String> {
        if core.pcm.is_empty() {
            return Ok(());
        }
        let range = Range {
            source,
            first: core.first,
            end: core.first + core.pcm.len() as u64 * 3,
        };
        if self.pending.len() >= MAX_PENDING
            || self.input.as_ref().is_none_or(|input| {
                input
                    .try_send(Window {
                        range,
                        pcm: core.pcm,
                    })
                    .is_err()
            })
        {
            // Capacity only recovers when poll drains completed requests.
            // Merge consecutively missed samples per source until that poll.
            let missed = self.missed[source as usize].get_or_insert(range);
            missed.end = range.end;
            return Err(format!(
                "Local speaker coverage unavailable for source {source}, frames [{}, {}).",
                range.first, range.end
            ));
        }
        self.pending.push_back(range);
        Ok(())
    }

    /// PCM is 16 kHz mono. The original source position is in 48 kHz frames.
    pub fn submit(&mut self, source: u32, first_frame: u64, pcm16: &[i16]) -> Result<(), String> {
        if self.failed || self.sealed {
            return Err("Local speaker input is closed.".into());
        }
        if source >= MAX_SOURCES
            || pcm16.len() > CORE
            || first_frame.checked_add(pcm16.len() as u64 * 3).is_none()
        {
            return Err("Local speaker input exceeds its source or window limit.".into());
        }
        if pcm16.is_empty() {
            return Ok(());
        }
        let cursor = &mut self.last_frame[source as usize];
        if cursor.is_some_and(|last| first_frame < last) {
            return Err("Speaker input overlaps earlier samples.".into());
        }
        *cursor = Some(first_frame + pcm16.len() as u64 * 3);
        let mut offset = first_frame;
        let mut pcm = pcm16;
        let mut error = None;
        while !pcm.is_empty() {
            let previous = self.cores.get(&source);
            if let Some(core) = previous {
                let end = core.first + core.pcm.len() as u64 * 3;
                if offset > end {
                    let old = self.cores.remove(&source).unwrap();
                    if let Err(e) = self.enqueue(source, old) {
                        error = Some(e);
                    }
                }
            }
            let core = self.cores.entry(source).or_insert_with(|| Core {
                first: offset,
                pcm: Vec::with_capacity(CORE),
            });
            let count = pcm.len().min(CORE - core.pcm.len());
            core.pcm.extend_from_slice(&pcm[..count]);
            offset += count as u64 * 3;
            pcm = &pcm[count..];
            if core.pcm.len() == CORE {
                let full = self.cores.remove(&source).unwrap();
                if let Err(e) = self.enqueue(source, full) {
                    error = Some(e);
                }
            }
        }
        error.map_or(Ok(()), Err)
    }

    pub fn seal(&mut self) -> Result<(), String> {
        self.sealed = true;
        let mut errors = Vec::new();
        for (source, core) in std::mem::take(&mut self.cores) {
            if let Err(e) = self.enqueue(source, core) {
                errors.push(e);
            }
        }
        self.input.take();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join(" "))
        }
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    pub fn failed(&self) -> bool {
        self.failed
    }

    pub fn poll(&mut self) -> Vec<SpeakerUpdate> {
        let mut updates: Vec<_> = self
            .missed
            .iter_mut()
            .filter_map(Option::take)
            .map(|range| {
                range.missing(
                    self.epoch,
                    "Local speaker queue could not accept this audio.",
                )
            })
            .collect();
        let Some(events) = self.events.as_ref() else {
            return updates;
        };
        let mut failure = None;
        loop {
            match events.try_recv() {
                Ok(Event::Result(update)) => {
                    self.pending.pop_front();
                    updates.push(update);
                }
                Ok(Event::Failed(why)) => {
                    failure = Some(why);
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if !self.sealed || !self.pending.is_empty() {
                        failure = Some("Local speaker adapter exited.".into());
                    }
                    break;
                }
            }
        }
        if let Some(why) = failure {
            self.failed = true;
            self.input.take();
            self.events.take();
            let mut lost: Vec<_> = self.pending.drain(..).collect();
            for (source, core) in std::mem::take(&mut self.cores) {
                lost.push(Range {
                    source,
                    first: core.first,
                    end: core.first + core.pcm.len() as u64 * 3,
                });
            }
            if lost.is_empty() {
                lost.push(Range {
                    source: 0,
                    first: 0,
                    end: 0,
                });
            }
            updates.extend(
                lost.into_iter()
                    .map(|range| range.missing(self.epoch, &why)),
            );
        }
        updates
    }
}

impl Drop for LiveSpeakers {
    fn drop(&mut self) {
        self.input.take();
        self.events.take();
        let _ = self.control.send(Watch::Stop);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn ready() -> String {
        serde_json::json!({
            "protocol": PROTOCOL, "ready": true, "backend": "fixture",
            "segmentation_sha256":"a".repeat(64), "embedding_sha256":"b".repeat(64),
            "threads":2, "threshold":0.5
        })
        .to_string()
            + "\n"
    }

    fn run_fixture(reply: String, source: u32) -> Result<(Vec<u8>, SpeakerUpdate), String> {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(Window {
            range: Range {
                source,
                first: 18,
                end: 24,
            },
            pcm: vec![1, -2],
        })
        .unwrap();
        drop(tx);
        let (out, events) = mpsc::sync_channel(1);
        let (watch, _r) = mpsc::channel();
        let mut bytes = Vec::new();
        exchange(
            &mut bytes,
            Cursor::new(ready() + &reply + "\n"),
            rx,
            &out,
            &watch,
            7,
            None,
        )?;
        match events.recv().unwrap() {
            Event::Result(update) => Ok((bytes, update)),
            Event::Failed(why) => Err(why),
        }
    }

    #[test]
    fn windows_preserve_source_time_overlap_and_epoch() {
        let reply = r#"{"source":1,"first_frame":18,"end_frame":24,"replace_from":18,"turns":[{"start":18,"end":23,"speaker":"3"},{"start":20,"end":24,"speaker":"1"}]}"#;
        let (wire, update) = run_fixture(reply.into(), 1).unwrap();
        let eol = wire.iter().position(|&x| x == b'\n').unwrap();
        let header: serde_json::Value = serde_json::from_slice(&wire[..eol]).unwrap();
        assert_eq!(header["samples"], 2);
        assert_eq!(header["first_frame"], 18);
        assert_eq!(&wire[eol + 1..], &[1, 0, 254, 255]);
        assert_eq!(update.turns[0].speaker, "live-7-1-3");
        assert_eq!(update.turns[1].start, 20);
        assert_eq!(update.backend.as_deref(), Some("fixture"));
        assert!(update.model.unwrap().contains(&"b".repeat(64)));
    }

    #[test]
    fn discards_mismatched_source_and_invalid_times() {
        let bad = r#"{"source":1,"first_frame":18,"end_frame":24,"replace_from":18,"turns":[]}"#;
        assert!(run_fixture(bad.into(), 0).is_err());
        let bad = r#"{"source":1,"first_frame":18,"end_frame":24,"replace_from":18,"turns":[{"start":17,"end":21,"speaker":"1"}]}"#;
        assert!(run_fixture(bad.into(), 1).is_err());
        assert!(line::<Ready>(&mut Cursor::new(vec![b'x'; MAX_REPLY + 2])).is_err());
    }

    #[test]
    fn unapproved_model_receives_no_audio() {
        let (input, windows) = mpsc::sync_channel(1);
        input
            .send(Window {
                range: Range {
                    source: 0,
                    first: 0,
                    end: 3,
                },
                pcm: vec![17],
            })
            .unwrap();
        drop(input);
        let (events, _) = mpsc::sync_channel(1);
        let (watch, _) = mpsc::channel();
        let profile = QualifiedProfile {
            bridge_sha256: "",
            backend: "fixture",
            segmentation_sha256: "wrong",
            embedding_sha256: "",
            threads: 2,
            threshold: 0.5,
        };
        let mut sent = Vec::new();
        assert_eq!(
            exchange(
                &mut sent,
                Cursor::new(ready()),
                windows,
                &events,
                &watch,
                1,
                Some(&profile)
            )
            .unwrap_err(),
            NO_PROFILE
        );
        assert!(sent.is_empty());
    }

    #[test]
    fn unavailable_release_profile_starts_no_process() {
        if !audio_capture::live_wire::QUALIFIED_LIVE_SPEAKERS {
            assert_eq!(
                LiveSpeakers::start(Path::new("missing-local-speaker-config.json"), 7)
                    .err()
                    .unwrap(),
                NO_PROFILE
            );
        }
    }

    #[test]
    fn speaker_admission_is_bounded_and_preserves_source_gaps() {
        let (input, rx) = mpsc::sync_channel(MAX_PENDING);
        let (_, events) = mpsc::sync_channel(MAX_PENDING);
        let (control, _) = mpsc::channel();
        let mut live = LiveSpeakers {
            epoch: 1,
            last_frame: [None; MAX_SOURCES as usize],
            cores: BTreeMap::new(),
            pending: VecDeque::new(),
            missed: [None; MAX_SOURCES as usize],
            input: Some(input),
            events: Some(events),
            control,
            threads: vec![],
            sealed: false,
            failed: false,
        };
        live.submit(0, 120, &[1, 2, 3]).unwrap();
        live.submit(0, 320, &[4, 5]).unwrap();
        let prefix = rx.recv().unwrap();
        assert_eq!((prefix.range.first, prefix.range.end), (120, 129));
        assert_eq!(prefix.pcm, vec![1, 2, 3]);
        let core = vec![1; CORE];
        for n in 0..MAX_PENDING - 1 {
            live.submit(1, (n * CORE * 3) as u64, &core).unwrap();
        }
        assert!(live.submit(1, 0, &core).is_err());
        let full = live
            .submit(1, ((MAX_PENDING - 1) * CORE * 3) as u64, &core)
            .unwrap_err();
        assert!(full.contains("coverage unavailable"));
        assert_eq!(live.pending(), MAX_PENDING);
        assert!(live.seal().is_err());
        assert!(live.submit(0, 326, &[6]).is_err());
        let gaps = live.poll();
        assert!(gaps.iter().any(|u| u.source == 1
            && u.first_frame == ((MAX_PENDING - 1) * CORE * 3) as u64
            && u.end_frame == (MAX_PENDING * CORE * 3) as u64
            && u.diagnostic.is_some()));
    }

    #[test]
    #[ignore = "requires installed local models, runtime and a speech PCM fixture"]
    fn real_model_seals_windows_and_releases_on_disable() {
        let config = PathBuf::from(std::env::var_os("AUDIO_CAPTURE_SPEAKER_CONFIG").unwrap());
        let raw = std::fs::read(std::env::var_os("AUDIO_CAPTURE_SPEAKER_PCM16").unwrap()).unwrap();
        let pcm: Vec<_> = raw
            .chunks_exact(2)
            .map(|s| i16::from_le_bytes([s[0], s[1]]))
            .collect();
        assert!((CORE + 1..CORE * 8).contains(&pcm.len()));
        let mut live = LiveSpeakers::start_evaluation(&config, 11).unwrap();
        for (i, block) in pcm.chunks(CORE).enumerate() {
            live.submit(0, (i * CORE * 3) as u64, block).unwrap();
        }
        live.seal().unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(45);
        let mut results = Vec::new();
        while live.pending() != 0 && std::time::Instant::now() < until {
            results.extend(live.poll());
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(live.pending(), 0);
        assert_eq!(results.len(), pcm.len().div_ceil(CORE));
        assert!(
            results
                .iter()
                .all(|u| u.diagnostic.is_none() && u.epoch == 11),
            "{results:?}"
        );
        assert!(
            results
                .iter()
                .flat_map(|u| &u.turns)
                .any(|t| t.speaker.starts_with("live-11-0-"))
        );
        assert!(
            results
                .windows(2)
                .any(|pair| pair[1].first_frame < pair[0].end_frame)
        );
        assert_eq!(results.last().unwrap().end_frame, pcm.len() as u64 * 3);
        drop(live);
        let fresh = LiveSpeakers::start_evaluation(&config, 12).unwrap();
        let start = std::time::Instant::now();
        drop(fresh);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
