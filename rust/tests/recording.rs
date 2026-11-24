#![cfg(windows)]

//! Opt-in integration test. This plays generated tones on the default audio
//! output and records only the two child processes created by this test.

use audio_capture::{
    capture::Input,
    record::{Config, Session},
};
use std::os::windows::ffi::OsStrExt;
use std::{
    f32::consts::PI,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[link(name = "winmm")]
unsafe extern "system" {
    fn PlaySoundW(sound: *const u16, module: isize, flags: u32) -> i32;
}

fn write_tone(path: &Path, frequency: f32) {
    let rate = 48_000_u32;
    let seconds = 16_u32;
    let sample_count = rate * seconds;
    let byte_count = sample_count * 2;
    let mut file = File::create(path).unwrap();
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + byte_count).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16_u32.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&rate.to_le_bytes()).unwrap();
    file.write_all(&(rate * 2).to_le_bytes()).unwrap();
    file.write_all(&2_u16.to_le_bytes()).unwrap();
    file.write_all(&16_u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&byte_count.to_le_bytes()).unwrap();
    for frame in 0..sample_count {
        let value = (0.20
            * i16::MAX as f32
            * (2.0 * PI * frequency * frame as f32 / rate as f32).sin()) as i16;
        file.write_all(&value.to_le_bytes()).unwrap();
    }
}

struct ToneChild(Child);

impl Drop for ToneChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn play_in_child(path: &Path) -> ToneChild {
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "tone_child", "--nocapture"])
        .env("AUDIO_CAPTURE_TEST_TONE", path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start isolated tone player");
    ToneChild(child)
}

#[test]
#[ignore = "helper process used by the audio isolation test"]
fn tone_child() {
    let Some(path) = std::env::var_os("AUDIO_CAPTURE_TEST_TONE") else {
        return;
    };
    let wide: Vec<u16> = path.encode_wide().chain(Some(0)).collect();
    // SND_FILENAME | SND_NODEFAULT, synchronous so the process owns playback.
    let ok = unsafe { PlaySoundW(wide.as_ptr(), 0, 0x0002_0000 | 0x0002) };
    assert_ne!(ok, 0, "Windows could not play the generated tone");
}

fn wav_file(folder: &Path) -> PathBuf {
    let files: Vec<_> = fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        })
        .collect();
    assert_eq!(
        files.len(),
        1,
        "one WAV file was expected in {folder:?}: {files:?}"
    );
    files[0].clone()
}

fn tone_levels(path: &Path) -> (f64, f64, f64) {
    let result = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "f32le", "-ac", "1", "-ar", "16000", "pipe:1"])
        .output()
        .expect("ffmpeg must be available on PATH");
    assert!(
        result.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let samples: Vec<f32> = result
        .stdout
        .chunks_exact(4)
        .map(|value| f32::from_le_bytes(value.try_into().unwrap()))
        .collect();
    assert!(
        samples.len() >= 16_000,
        "recording must contain at least one second of audio"
    );
    let rate = 16_000.0_f64;
    let strength = |frequency: f64| {
        let (mut real, mut imaginary) = (0.0_f64, 0.0_f64);
        for (index, &sample) in samples.iter().enumerate() {
            let angle = 2.0 * std::f64::consts::PI * frequency * index as f64 / rate;
            real += sample as f64 * angle.cos();
            imaginary += sample as f64 * angle.sin();
        }
        2.0 * (real * real + imaginary * imaginary).sqrt() / samples.len() as f64
    };
    (
        strength(700.0),
        strength(1300.0),
        samples.len() as f64 / rate,
    )
}

fn config(folder: PathBuf, pid: u32) -> Config {
    Config {
        inputs: vec![("tone".into(), Input::Process(pid), 1.0)],
        folder,
        format: "wav".into(),
        rate: 16_000,
        channels: 1,
        bitrate: 32,
        flac_level: 5,
        separate: true,
        mixed: false,
        skip_silence: false,
        monitor: None,
        monitor_only: false,
    }
}

#[test]
#[ignore = "plays two audible test tones; requires a working Windows output and ffmpeg"]
fn selected_process_stays_isolated_across_sessions() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("audio-isolation-{}-{stamp}", std::process::id()));
    let first = root.join("first");
    let second = root.join("second");
    let both = root.join("both");
    let dead = root.join("dead");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    fs::create_dir_all(&both).unwrap();
    fs::create_dir_all(&dead).unwrap();
    let tone_a = root.join("tone-700.wav");
    let tone_b = root.join("tone-1300.wav");
    write_tone(&tone_a, 700.0);
    write_tone(&tone_b, 1300.0);
    let mut player_a = play_in_child(&tone_a);
    let mut player_b = play_in_child(&tone_b);
    thread::sleep(Duration::from_millis(1500));
    assert!(
        player_a.0.try_wait().unwrap().is_none(),
        "first tone player exited early"
    );
    assert!(
        player_b.0.try_wait().unwrap().is_none(),
        "second tone player exited early"
    );

    let mut one = Session::start(config(first.clone(), player_a.0.id()))
        .expect("start first selected-process capture");
    thread::sleep(Duration::from_secs(3));
    one.stop().expect("finalize first recording");

    let mut two = Session::start(config(second.clone(), player_b.0.id()))
        .expect("start second selected-process capture");
    thread::sleep(Duration::from_secs(3));
    two.stop().expect("finalize second recording");

    let mut combined_config = config(both.clone(), player_a.0.id());
    combined_config.inputs = vec![
        ("tone-700".into(), Input::Process(player_a.0.id()), 1.0),
        ("tone-1300".into(), Input::Process(player_b.0.id()), 1.0),
    ];
    combined_config.mixed = true;
    let mut three = Session::start(combined_config).expect("start mixed and isolated capture");
    thread::sleep(Duration::from_millis(1200));
    three.pause(true);
    thread::sleep(Duration::from_millis(600));
    three.pause(false);
    thread::sleep(Duration::from_millis(1200));
    three.stop().expect("finalize mixed and isolated recording");

    let mut exited = Command::new("cmd.exe")
        .args(["/c", "exit", "0"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start short-lived process");
    let dead_pid = exited.id();
    exited.wait().unwrap();
    let mut unavailable =
        Session::start(config(dead, dead_pid)).expect("start session with a terminated process");
    for _ in 0..50 {
        if unavailable.is_finished() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        unavailable.is_finished(),
        "terminated source must report an error"
    );
    let error = unavailable.stop().expect_err("terminated source must fail");
    eprintln!("terminated source returned: {error}");

    let (a_700, a_1300, a_duration) = tone_levels(&wav_file(&first));
    let (b_700, b_1300, b_duration) = tone_levels(&wav_file(&second));
    eprintln!("first: 700={a_700:.4}, 1300={a_1300:.4}, seconds={a_duration:.2}");
    eprintln!("second: 700={b_700:.4}, 1300={b_1300:.4}, seconds={b_duration:.2}");
    assert!(
        (1.0..=5.0).contains(&a_duration) && (1.0..=5.0).contains(&b_duration),
        "recorded duration is outside the expected range"
    );
    assert!(
        a_700 > 0.01 && a_700 > a_1300 * 8.0,
        "first file did not isolate the 700 Hz source"
    );
    assert!(
        b_1300 > 0.01 && b_1300 > b_700 * 8.0,
        "second file did not isolate the 1300 Hz source"
    );
    let both_files: Vec<_> = fs::read_dir(&both)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        })
        .collect();
    assert_eq!(
        both_files.len(),
        3,
        "both mode must produce three WAV files"
    );
    let find = |prefix: &str| {
        both_files
            .iter()
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(prefix)
            })
            .unwrap_or_else(|| panic!("missing {prefix} track in {both_files:?}"))
    };
    let (mix_a, mix_b, mix_duration) = tone_levels(find("Combined-"));
    let (a_a, a_b, a_track_duration) = tone_levels(find("tone-700-"));
    let (b_a, b_b, b_track_duration) = tone_levels(find("tone-1300-"));
    eprintln!(
        "both: combined 700={mix_a:.4}, 1300={mix_b:.4}, seconds={mix_duration:.2}; isolated 700={a_a:.4}/{a_b:.4}, 1300={b_a:.4}/{b_b:.4}"
    );
    assert!(
        mix_a > 0.01 && mix_b > 0.01,
        "mixed file must contain both sources"
    );
    assert!(a_a > 0.01 && a_a > a_b * 8.0, "first isolated track leaked");
    assert!(
        b_b > 0.01 && b_b > b_a * 8.0,
        "second isolated track leaked"
    );
    assert!((mix_duration - a_track_duration).abs() < 0.05);
    assert!((mix_duration - b_track_duration).abs() < 0.05);
}
