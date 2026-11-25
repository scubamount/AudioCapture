#![cfg(windows)]

//! Opt-in acceptance using two generated playback processes and a local HTTP endpoint.

use audio_capture::{
    capture::Input,
    live_wire::{launch_relay, new_interval_id},
    record::{Config, LiveTap, Session},
};
use serde_json::{Value, json};
use std::{
    error::Error,
    ffi::OsStr,
    fs::{self, File},
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RATE: u64 = 48_000;
const LABELS: [&str; 2] = ["Generated-700", "Generated-1300"];
const FREQUENCIES: [f64; 2] = [700.0, 1300.0];
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(25);
const TONE_ENV: &str = "AUDIO_CAPTURE_LIVE_TEST_TONE";

type AnyResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[link(name = "winmm")]
unsafe extern "system" {
    fn PlaySoundW(sound: *const u16, module: isize, flags: u32) -> i32;
}

fn write_tone(path: &Path, frequency: f64) {
    let mut file = BufWriter::new(File::create(path).unwrap());
    let rate = RATE as u32;
    let samples = rate * 60;
    let bytes = samples * 2;
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16_u32.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&rate.to_le_bytes()).unwrap();
    file.write_all(&(rate * 2).to_le_bytes()).unwrap();
    file.write_all(&2_u16.to_le_bytes()).unwrap();
    file.write_all(&16_u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&bytes.to_le_bytes()).unwrap();
    for frame in 0..samples {
        let audible = frame % (rate * 3) < rate * 2;
        let angle = std::f64::consts::TAU * frequency * f64::from(frame) / f64::from(rate);
        let sample = if audible {
            (5_000.0 * angle.sin()) as i16
        } else {
            0
        };
        file.write_all(&sample.to_le_bytes()).unwrap();
    }
    file.flush().unwrap();
}

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn play(path: &Path) -> Process {
    Process(
        Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "live_session_tone_child"])
            .env(TONE_ENV, path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

#[test]
#[ignore = "helper process for the generated-audio acceptance test"]
fn live_session_tone_child() {
    if let Some(path) = std::env::var_os(TONE_ENV) {
        let wide: Vec<u16> = path.encode_wide().chain(Some(0)).collect();
        // SND_FILENAME | SND_NODEFAULT. Playback belongs to this process.
        let ok = unsafe { PlaySoundW(wide.as_ptr(), 0, 0x0002_0000 | 0x0002) };
        assert_ne!(ok, 0, "Windows could not play generated audio");
    }
}

fn pcm_wav(bytes: &[u8]) -> AnyResult<(u32, &[u8])> {
    if bytes.len() < 44
        || !bytes.starts_with(b"RIFF")
        || &bytes[8..16] != b"WAVEfmt "
        || &bytes[20..24] != [1, 0, 1, 0]
        || &bytes[34..40] != b"\x10\0data"
    {
        return Err("Expected a mono PCM16 RIFF/WAVE".into());
    }
    let rate = u32::from_le_bytes(bytes[24..28].try_into()?);
    let size = u32::from_le_bytes(bytes[40..44].try_into()?) as usize;
    let data = bytes.get(44..44 + size).ok_or("Incomplete WAV payload")?;
    if rate == 0 || data.len() % 2 != 0 {
        return Err("Invalid WAV sample rate or alignment".into());
    }
    Ok((rate, data))
}

fn strengths(rate: u32, pcm: &[u8]) -> [f64; 2] {
    let n = (pcm.len() / 2).max(1) as f64;
    FREQUENCIES.map(|frequency| {
        let dt = std::f64::consts::TAU * frequency / f64::from(rate);
        let (x, y) = pcm
            .chunks_exact(2)
            .enumerate()
            .fold((0.0, 0.0), |(x, y), (i, frame)| {
                let (sin, cos) = (i as f64 * dt).sin_cos();
                let amplitude = f64::from(i16::from_le_bytes([frame[0], frame[1]]));
                (x + cos * amplitude, y + sin * amplitude)
            });
        2.0 * x.hypot(y) / n
    })
}

#[derive(Default)]
struct MockStats {
    requests: [usize; 2],
    quiet: usize,
    errors: Vec<String>,
}

struct Mock {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    stats: Arc<Mutex<MockStats>>,
    thread: Option<JoinHandle<()>>,
}

impl Mock {
    fn start() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Mutex::new(MockStats::default()));
        let halted = Arc::clone(&stop);
        let counts = Arc::clone(&stats);
        let thread = thread::spawn(move || {
            while !halted.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if let Err(error) = respond(stream, &counts) {
                            eprintln!("Loopback request rejected: {error}");
                            counts.lock().unwrap().errors.push(error.to_string());
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => {
                        counts.lock().unwrap().errors.push(error.to_string());
                        break;
                    }
                }
            }
        });
        Self {
            address,
            stop,
            stats,
            thread: Some(thread),
        }
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.thread.take().unwrap().join();
    }
}

fn respond(mut stream: TcpStream, stats: &Mutex<MockStats>) -> AnyResult<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(&mut stream);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    if !request.starts_with("POST /bench/v1/audio/transcriptions HTTP/") {
        return Err(format!("Unexpected mock request {request:?}").into());
    }
    let mut length = None;
    let mut header = String::new();
    loop {
        header.clear();
        if reader.read_line(&mut header)? == 0 {
            return Err("Missing HTTP body".into());
        }
        if header == "\r\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<usize>()?);
        }
    }
    let length = length
        .filter(|n| *n < 512 * 1024)
        .ok_or("Missing or excessive multipart size")?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let start = body
        .windows(4)
        .position(|value| value == b"RIFF")
        .ok_or("Missing WAV upload")?;
    let (rate, pcm) = pcm_wav(&body[start..])?;
    if u64::from(rate) != RATE || pcm.len() / 2 > 4 * RATE as usize {
        return Err("Unexpected live sample format or chunk duration".into());
    }
    let values = strengths(rate, pcm);
    let mut observed = stats.lock().unwrap();
    let text = if values[0] > 100.0 && values[0] > values[1] * 8.0 {
        observed.requests[0] += 1;
        "isolated seven hundred"
    } else if values[1] > 100.0 && values[1] > values[0] * 8.0 {
        observed.requests[1] += 1;
        "isolated thirteen hundred"
    } else if values.iter().all(|n| *n < 100.0) {
        observed.quiet += 1;
        ""
    } else {
        return Err(format!("Live request mixes the two generated sources: {values:?}").into());
    };
    drop(observed);
    // Slower than a simple in-memory response, so audio capture must proceed concurrently.
    thread::sleep(Duration::from_millis(350));
    let body = json!({"text": text}).to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()?;
    Ok(())
}

struct Worker {
    process: Process,
    output: Receiver<Value>,
    log: PathBuf,
    id: String,
}

impl Worker {
    fn launch(executable: &Path, mock: &Mock, root: &Path) -> (Self, Box<dyn LiveTap>) {
        let id = new_interval_id();
        let log = root.join(format!("{id}.log"));
        let isolated = root.join("localappdata");
        fs::create_dir_all(&isolated).unwrap();
        let endpoint = format!("http://{}/bench/v1", mock.address);
        let process = Command::new(executable)
            .args([
                "live",
                "--confirmed",
                "--base-url",
                &endpoint,
                "--model",
                "generated",
            ])
            .args(["--interval-id", &id, "--output-dir"])
            .arg(root)
            .args(["--source", LABELS[0], "--source", LABELS[1]])
            .env("LOCALAPPDATA", &isolated)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(File::create(&log).unwrap())
            .spawn()
            .unwrap();
        let mut process = Process(process);
        let stdin = process.0.stdin.take().unwrap();
        let reader = BufReader::new(process.0.stdout.take().unwrap());
        let (tx, output) = mpsc::channel();
        thread::spawn(move || {
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if let Ok(status) = serde_json::from_str(&line)
                    && tx.send(status).is_err()
                {
                    break;
                }
            }
        });
        let worker = Self {
            process,
            output,
            log,
            id,
        };
        worker.wait_status(&["ready"], READY_TIMEOUT);
        let relay =
            launch_relay(stdin, worker.id.clone(), LABELS.map(str::to_owned).to_vec()).unwrap();
        (worker, relay)
    }

    fn wait_status(&self, wanted: &[&str], timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        let mut seen = Vec::new();
        while let Ok(value) = self
            .output
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            if value["state"]
                .as_str()
                .is_some_and(|state| wanted.contains(&state))
            {
                return value;
            }
            seen.push(value);
        }
        panic!(
            "No worker state {wanted:?}. Received {seen:?}. Worker log: {}",
            self.log.display()
        );
    }

    fn finalized(&mut self) -> Transcript {
        let status = self.wait_status(&["complete", "incomplete"], OUTPUT_TIMEOUT);
        let text_path = PathBuf::from(status["path"].as_str().unwrap());
        let text = fs::read_to_string(&text_path).unwrap();
        let index = entries(&text_path.with_extension("segments.jsonl"));
        wait_until("worker exits", Duration::from_secs(3), || {
            self.process.0.try_wait().unwrap().is_some()
        });
        assert!(self.process.0.wait().unwrap().success());
        let end = index.last().unwrap();
        assert_eq!(end["type"], "end", "{index:?}");
        eprintln!(
            "Worker {}: {} chunks with speech, {} gap rows, cutoff frame {}",
            self.id, end["speech_entries"], end["gaps"], end["cutoff_frame"]
        );
        Transcript {
            text,
            index,
            path: text_path,
        }
    }
}

struct Transcript {
    text: String,
    index: Vec<Value>,
    path: PathBuf,
}

impl Transcript {
    fn cutoff(&self) -> u64 {
        self.index.last().unwrap()["cutoff_frame"].as_u64().unwrap()
    }

    fn check_sources(&self) {
        for (source, spoken) in LABELS
            .into_iter()
            .zip(["isolated seven hundred", "isolated thirteen hundred"])
        {
            assert!(
                self.text.contains(source) && self.text.contains(spoken),
                "Missing {source} in {}",
                self.path.display()
            );
            assert!(
                self.index.iter().any(|row| row["type"] == "speech"
                    && row["source"] == source
                    && row["text"] == spoken),
                "Missing assigned {source} phrase: {:?}",
                self.index
            );
        }
    }
}

fn entries(path: &Path) -> Vec<Value> {
    BufReader::new(File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect()
}

fn files(folder: &Path, suffix: &str) -> Vec<PathBuf> {
    fs::read_dir(folder)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .ends_with(suffix)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn wait_until(label: &str, timeout: Duration, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !ready() {
        assert!(Instant::now() < deadline, "Timed out waiting for {label}");
        thread::sleep(Duration::from_millis(25));
    }
}

fn config(folder: PathBuf, sources: &[Process; 2]) -> Config {
    Config {
        inputs: LABELS
            .into_iter()
            .zip(sources)
            .map(|(name, player)| (name.into(), Input::Process(player.0.id()), 1.0))
            .collect(),
        folder,
        format: "wav".into(),
        rate: 16_000,
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

fn timeline(folder: &Path, count: usize) -> Vec<(PathBuf, Vec<Value>)> {
    wait_until("closed timeline", Duration::from_secs(5), || {
        files(folder, ".timeline.jsonl").len() == count
    });
    let mut timelines: Vec<_> = files(folder, ".timeline.jsonl")
        .into_iter()
        .map(|path| {
            let rows = entries(&path);
            (path, rows)
        })
        .collect();
    timelines.sort_by_key(|(_, rows)| rows.first().unwrap()["start"].as_u64().unwrap());
    timelines
}

fn check_recording(folder: &Path, rows: &[Value], interval: &str) -> u64 {
    let header = rows.first().unwrap();
    let end = rows.last().unwrap();
    assert_eq!(end["type"], "end");
    assert_eq!(end["complete"], true, "{rows:?}");
    assert_eq!(header["sources"], json!(LABELS));
    let recording = header["recording"].as_str().unwrap();
    assert!(
        rows.iter()
            .any(|row| row["type"] == "transcript" && row["interval"] == interval),
        "{rows:?}"
    );
    let saved = end["saved_frames"].as_u64().unwrap();
    let cutoff = end["end"].as_u64().unwrap();
    assert!(saved < cutoff - header["start"].as_u64().unwrap());
    let mut session_frame = header["mapped_from"].as_u64().unwrap();
    let mut file_frame = header["saved_at_mapped_from"].as_u64().unwrap();
    for span in rows.iter().filter(|row| row["type"] == "span") {
        assert_eq!(span["start"], session_frame, "{span:?}");
        assert_eq!(span["saved"], file_frame, "{span:?}");
        let next = span["end"].as_u64().unwrap();
        assert!(next > session_frame);
        match span["kind"].as_str().unwrap() {
            "saved" => file_frame += next - session_frame,
            "pause" | "silence" => {}
            kind => panic!("Unknown recording span type {kind}"),
        }
        session_frame = next;
    }
    assert_eq!(session_frame, cutoff, "{rows:?}");
    assert_eq!(file_frame, saved, "{rows:?}");
    for row in header["outputs"].as_array().unwrap() {
        let source = row["source"].as_u64().unwrap() as usize;
        let filename = row["file"].as_str().unwrap();
        let wav = fs::read(folder.join(filename)).unwrap();
        let (rate, data) = pcm_wav(&wav).unwrap();
        let strength = strengths(rate, data);
        assert!(
            strength[source] > 100.0 && strength[source] > 8.0 * strength[1 - source],
            "{filename}: {strength:?}"
        );
        assert!(
            (data.len() as u64 / 2).abs_diff(saved * u64::from(rate) / RATE) <= 1,
            "{filename}: incorrect sample count for {recording}"
        );
    }
    cutoff
}

#[test]
#[ignore = "plays two generated tones on a working Windows output and starts a local mock endpoint"]
fn live_session_real_windows_acceptance() {
    let executable = std::env::var_os("AUDIO_CAPTURE_LIVE_TEST_WORKER")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_transcribe")));
    assert!(
        executable.is_file(),
        "No worker at {}",
        executable.display()
    );
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let scratch = std::env::var_os("AUDIO_CAPTURE_LIVE_TEST_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = scratch.join(format!("live-session-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let root = std::path::absolute(root).unwrap();
    eprintln!("Evidence directory {}", root.display());
    let recording_dir = root.join("recordings");
    let paths = LABELS.map(|name| root.join(OsStr::new(&format!("{name}.wav"))));
    for (path, frequency) in paths.iter().zip(FREQUENCIES) {
        write_tone(path, frequency);
    }
    let mut players = paths.each_ref().map(|path| play(path));
    thread::sleep(Duration::from_millis(800));
    assert!(
        players
            .iter_mut()
            .all(|p| p.0.try_wait().unwrap().is_none())
    );
    let mock = Mock::start();

    let (mut live_a, tap_a) = Worker::launch(&executable, &mock, &root);
    let mut session = Session::start_live(config(recording_dir.clone(), &players), tap_a).unwrap();
    let active = live_a.wait_status(&["active"], READY_TIMEOUT);
    assert!(session.transcribing() && !session.recording());
    assert!(files(&recording_dir, ".wav").is_empty());
    // Commit text while the session still has no audio writer.
    let partial = PathBuf::from(active["path"].as_str().unwrap());
    wait_until("readable audio-free live transcript", READY_TIMEOUT, || {
        fs::read_to_string(&partial).is_ok_and(|text| {
            text.contains("isolated seven hundred") && text.contains("isolated thirteen hundred")
        })
    });
    assert!(files(&recording_dir, ".wav").is_empty());
    session.start_recording().unwrap();
    wait_until("two wav tracks", READY_TIMEOUT, || {
        files(&recording_dir, ".wav").len() == 2
    });
    thread::sleep(Duration::from_millis(1200));
    session.pause(true);
    thread::sleep(Duration::from_millis(750));
    session.pause(false);
    thread::sleep(Duration::from_millis(2600));
    session.stop_recording().unwrap();
    assert!(session.transcribing() && !session.recording());
    let traces = timeline(&recording_dir, 1);
    let first = &traces[0].1;
    assert!(first.iter().any(|row| row["kind"] == "pause"), "{first:?}");
    assert!(
        first.iter().any(|row| row["kind"] == "silence"),
        "{first:?}"
    );
    let first_cutoff = check_recording(&recording_dir, first, &live_a.id);
    thread::sleep(Duration::from_millis(2500));

    // Start another audio interval, then stop only the first live worker.
    session.start_recording().unwrap();
    wait_until("second two wav tracks", READY_TIMEOUT, || {
        files(&recording_dir, ".wav").len() == 4
    });
    thread::sleep(Duration::from_millis(800));
    session.stop_live().unwrap();
    let transcript_a = live_a.finalized();
    transcript_a.check_sources();
    assert!(transcript_a.cutoff() > first_cutoff + RATE);
    assert!(transcript_a.index.iter().any(|row| {
        row["type"] == "speech"
            && row["end_frame"]
                .as_u64()
                .is_some_and(|end| end > first_cutoff)
    }));
    assert!(session.recording() && !session.transcribing() && !session.is_finished());

    let (mut live_b, tap_b) = Worker::launch(&executable, &mock, &root);
    session.attach_live(tap_b).unwrap();
    live_b.wait_status(&["active"], READY_TIMEOUT);
    thread::sleep(Duration::from_millis(5200));
    session.stop_both().unwrap();
    wait_until("both capture activities stop", READY_TIMEOUT, || {
        session.is_finished()
    });
    session.stop().unwrap();
    let transcript_b = live_b.finalized();
    transcript_b.check_sources();
    let traces = timeline(&recording_dir, 2);
    let second = &traces[1].1;
    assert_ne!(first[0]["recording"], second[0]["recording"]);
    assert_ne!(transcript_a.path, transcript_b.path);
    assert_eq!(
        transcript_a.index[0]["session"],
        transcript_b.index[0]["session"]
    );
    assert_eq!(transcript_b.index[0]["session"], session.session_id());
    let paired_cutoff = check_recording(&recording_dir, second, &live_b.id);
    assert_eq!(paired_cutoff, transcript_b.cutoff());
    assert!(transcript_b.index.iter().any(|row| {
        row["type"] == "recording"
            && row["id"] == second[0]["recording"]
            && row["end_frame"] == paired_cutoff
    }));
    eprintln!(
        "Shared capture and two independent stop orders passed. {}",
        session.status()
    );

    // A destination which is a file reliably fails the recording writer open.
    let invalid_destination = root.join("cannot-open-recording-folder");
    fs::write(&invalid_destination, b"leave this file intact").unwrap();
    let failure_dir = root.join("failure");
    let (mut live_c, tap_c) = Worker::launch(&executable, &mock, &failure_dir);
    let mut session =
        Session::start_live(config(invalid_destination.clone(), &players), tap_c).unwrap();
    let status = live_c.wait_status(&["active"], READY_TIMEOUT);
    session.start_recording().unwrap();
    wait_until("recording open failure", READY_TIMEOUT, || {
        !session.recording()
    });
    assert!(session.transcribing() && !session.is_finished());
    let partial = PathBuf::from(status["path"].as_str().unwrap());
    wait_until(
        "both sources reach text after the writer fails",
        READY_TIMEOUT,
        || {
            fs::read_to_string(&partial).is_ok_and(|text| {
                text.contains("isolated seven hundred")
                    && text.contains("isolated thirteen hundred")
            })
        },
    );
    players[0].0.kill().unwrap();
    players[0].0.wait().unwrap();
    wait_until("the first source reports loss", READY_TIMEOUT, || {
        session
            .levels()
            .lines()
            .any(|line| line.starts_with(LABELS[0]) && line.contains("source stopped"))
    });
    assert!(session.transcribing() && !session.is_finished());
    thread::sleep(Duration::from_millis(1800));
    session.stop_live().unwrap();
    session.stop().unwrap();
    let transcript_c = live_c.finalized();
    transcript_c.check_sources();
    assert_ne!(
        transcript_c.index[0]["session"],
        transcript_a.index[0]["session"]
    );
    let first_loss = transcript_c
        .index
        .iter()
        .find(|row| {
            row["type"] == "gap"
                && row["source"] == LABELS[0]
                && row["reason"] == "Capture source unavailable"
        })
        .expect("the transcript must identify the source that stopped");
    let loss_start = first_loss["start_frame"].as_u64().unwrap();
    assert!(transcript_c.index.iter().any(|row| {
        row["type"] == "speech"
            && row["source"] == LABELS[1]
            && row["end_frame"]
                .as_u64()
                .is_some_and(|end| end > loss_start)
    }));
    assert_eq!(
        fs::read(&invalid_destination).unwrap(),
        b"leave this file intact"
    );
    assert!(files(&failure_dir, ".wav").is_empty());

    let stats = mock.stats.lock().unwrap();
    assert!(stats.errors.is_empty(), "{:?}", stats.errors);
    assert!(
        stats.requests.iter().all(|&n| n >= 3),
        "{:?}",
        stats.requests
    );
    eprintln!(
        "Loopback requests by selected source: {:?}. Quiet requests: {}",
        stats.requests, stats.quiet
    );
}
