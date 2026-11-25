#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod native_controller;
mod native_layout;
mod native_live;
mod native_window;
mod worker_launch;

fn check_transcription_config(mut args: std::env::ArgsOs) -> Result<(), String> {
    use audio_capture::transcription_config::{FILENAME, TranscriptionConfig};
    if let Some(path) = args.next() {
        if args.next().is_some() {
            return Err("Usage: audio-capture --check-transcription-config [FILE]".into());
        }
        TranscriptionConfig::load(std::path::Path::new(&path))?;
    } else {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        TranscriptionConfig::load_optional_next_to(&exe)?
            .ok_or_else(|| format!("No {FILENAME} next to the recorder."))?;
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    let mut args = std::env::args_os();
    args.next();
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--check-transcription-config")) {
        return match check_transcription_config(args) {
            Ok(()) => {
                println!("Transcription config is valid.");
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if let Err(error) = native_window::run() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        let message: Vec<u16> = error.encode_utf16().chain(Some(0)).collect();
        let title: Vec<u16> = "Audio Capture".encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            )
        };
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
