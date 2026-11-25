//! Recording-only benchmark driver for tools/test_live_performance.py.
//! Each input must be a dedicated synthetic player. No microphone or system route is opened.

use audio_capture::capture::Input;
use audio_capture::record::{Config, Session};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let folder = PathBuf::from(args.next().ok_or("Missing output directory")?);
    let seconds: u64 = args
        .next()
        .and_then(|value| value.to_str()?.parse().ok())
        .ok_or("Missing duration in seconds")?;
    let mut inputs = Vec::new();
    for label in ["program", "microphone-fixture"] {
        let pid: u32 = args
            .next()
            .and_then(|value| value.to_str()?.parse().ok())
            .ok_or("Expected two synthetic player PIDs")?;
        inputs.push((label.to_string(), Input::Process(pid), 1.0));
    }
    if args.next().is_some() || !(1..=86_400).contains(&seconds) {
        return Err("Expected output directory, duration (1-86400), and two PIDs".into());
    }
    let mut session = Session::start(Config {
        inputs,
        folder,
        format: "opus".into(),
        rate: 48_000,
        channels: 1,
        bitrate: 32,
        flac_level: 5,
        separate: true,
        mixed: false,
        skip_silence: false,
        monitor_only: false,
        monitor: None,
    })?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !session.status().contains("Recording") {
        if session.is_finished() || Instant::now() >= deadline {
            let status = session.status();
            session.stop()?;
            return Err(format!(
                "Could not start synthetic process capture: {status}"
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("READY");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let started = Instant::now();
    let duration = Duration::from_secs(seconds);
    while started.elapsed() < duration && !session.is_finished() {
        std::thread::sleep(
            duration
                .saturating_sub(started.elapsed())
                .min(Duration::from_millis(100)),
        );
    }
    session.stop()?;
    println!("{}", session.status());
    if started.elapsed() < duration {
        return Err("Synthetic capture ended before its deadline".into());
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
