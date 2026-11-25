#![windows_subsystem = "windows"]

mod native_controller;
mod native_layout;
mod native_live;
mod native_window;
mod worker_launch;

fn run_windowless_worker(
    command: &str,
    args: impl Iterator<Item = std::ffi::OsString>,
) -> std::process::ExitCode {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, ExitCode, Stdio};
    use windows_sys::Win32::System::Threading::DETACHED_PROCESS;

    let status = std::env::current_exe().and_then(|exe| {
        Command::new(exe.with_file_name("transcribe.exe"))
            .arg(command)
            .args(args)
            .creation_flags(DETACHED_PROCESS)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
    });
    match status {
        Ok(status) => ExitCode::from(
            status
                .code()
                .and_then(|code| code.try_into().ok())
                .unwrap_or(1),
        ),
        Err(_) => {
            let message = "Cannot start transcribe.exe next to Audio Capture.";
            if command == "control" {
                println!(
                    "{}",
                    serde_json::json!({"ok":false,"schema_version":1,
                        "error":{"code":"operation_failed","message":message}})
                );
            } else {
                eprintln!("{message}");
            }
            ExitCode::FAILURE
        }
    }
}

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
    let command = args.next();
    if matches!(
        command.as_deref().and_then(std::ffi::OsStr::to_str),
        Some("mcp" | "--mcp")
    ) {
        return run_windowless_worker("mcp", args);
    }
    if command.as_deref() == Some(std::ffi::OsStr::new("control")) {
        return run_windowless_worker("control", args);
    }
    if command.as_deref() == Some(std::ffi::OsStr::new("--check-transcription-config")) {
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
