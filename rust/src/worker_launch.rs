use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use audio_capture::transcription_config::{
    GUI_CONFIG_ENV, GUI_PID_ENV, TranscriptionConfig, is_gui_environment_variable,
};
use windows::Win32::Foundation::{E_INVALIDARG, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
use windows::Win32::System::Threading::{
    CREATE_NEW_CONSOLE, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, PROCESS_INFORMATION,
    STARTUPINFOW,
};
use windows::core::{HRESULT, PCWSTR, PWSTR};

const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;

pub(crate) fn launch_transcription(
    files: &[PathBuf],
    config: Option<&TranscriptionConfig>,
) -> Result<(), String> {
    let snapshot = config
        .map(TranscriptionConfig::to_gui_snapshot)
        .transpose()?;
    launch_console(
        "--guided --",
        files,
        Some(snapshot.as_deref().unwrap_or("")),
    )
}

pub(crate) fn launch_batch_manager() -> Result<(), String> {
    launch_console("batch-queue manage", &[], None)
}

fn launch_console(mode: &str, files: &[PathBuf], snapshot: Option<&str>) -> Result<(), String> {
    let recorder = std::env::current_exe()
        .map_err(|error| format!("Could not find the recorder executable. {error}"))?;
    let worker = recorder.with_file_name("transcribe.exe");
    start_worker(&worker, mode, files, snapshot).map_err(|error| {
        let worker = worker.display();
        let code = error.code();
        if [ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND]
            .into_iter()
            .any(|missing| code == HRESULT::from_win32(missing.0))
        {
            format!("Transcription worker not found at {worker}. Install transcribe.exe next to the recorder.")
        } else {
            format!("Could not start the transcription worker at {worker}. {error}")
        }
    })
}

fn start_worker(
    worker: &Path,
    mode: &str,
    files: &[PathBuf],
    snapshot: Option<&str>,
) -> windows::core::Result<()> {
    let application: Vec<_> = worker.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut command_line = worker_command_line(worker, mode, files)?;
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    let environment = gui_environment(std::env::vars_os(), std::process::id(), snapshot);

    // Let Windows connect all three standard handles to the new console.
    // The UTF-16 strings are terminated and the command line can be modified.
    unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_NEW_CONSOLE | CREATE_UNICODE_ENVIRONMENT,
            Some(environment.as_ptr().cast()),
            PCWSTR::null(),
            &startup,
            &mut process,
        )?;
        // CreateProcessW transfers ownership of both handles on success.
        drop(OwnedHandle::from_raw_handle(process.hThread.0));
        drop(OwnedHandle::from_raw_handle(process.hProcess.0));
    }
    Ok(())
}

// Attach the recorder lifetime to this guided batch without changing the parent environment.
fn gui_environment(
    vars: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    pid: u32,
    snapshot: Option<&str>,
) -> Vec<u16> {
    let mut vars: Vec<_> = vars
        .filter(|(name, _)| !is_gui_environment_variable(name))
        .collect();
    vars.push((GUI_PID_ENV.into(), pid.to_string().into()));
    if let Some(snapshot) = snapshot {
        vars.push((GUI_CONFIG_ENV.into(), snapshot.into()));
    }
    vars.sort_by_cached_key(|(name, _)| name.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (name, value) in vars {
        block.extend(name.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

fn worker_command_line(
    worker: &Path,
    mode: &str,
    files: &[PathBuf],
) -> windows::core::Result<Vec<u16>> {
    // The executable path is argv[0]. Windows file names cannot contain quotes.
    let mut command_line = vec![QUOTE];
    command_line.extend(worker.as_os_str().encode_wide());
    command_line.push(QUOTE);
    command_line.push(b' ' as u16);
    command_line.extend(mode.encode_utf16());
    for file in files {
        append_argument(&mut command_line, file.as_os_str());
    }
    if command_line.contains(&0) {
        return Err(E_INVALIDARG.into());
    }
    command_line.push(0);
    Ok(command_line)
}

fn append_argument(command_line: &mut Vec<u16>, argument: &OsStr) {
    command_line.extend([b' ' as u16, QUOTE]);
    let mut backslashes = 0;
    for unit in argument.encode_wide() {
        if unit == BACKSLASH {
            backslashes += 1;
            continue;
        }
        // Double backslashes before a quote, then escape the quote itself.
        if unit == QUOTE {
            backslashes = 2 * backslashes + 1;
        }
        command_line.extend(std::iter::repeat_n(BACKSLASH, backslashes));
        command_line.push(unit);
        backslashes = 0;
    }
    command_line.extend(std::iter::repeat_n(BACKSLASH, 2 * backslashes));
    command_line.push(QUOTE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows::core::PWSTR;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;

    #[test]
    fn guided_child_receives_its_parent_once_without_changing_other_values() {
        let env = gui_environment(
            [
                ("A".into(), "left=right".into()),
                ("audiocapture_gui_pid".into(), "old".into()),
                ("audiocapture_gui_config".into(), "old config".into()),
            ]
            .into_iter(),
            92,
            Some(r#"{"model":"current"}"#),
        );
        assert_eq!(
            String::from_utf16(&env).unwrap(),
            "A=left=right\0AUDIOCAPTURE_GUI_CONFIG={\"model\":\"current\"}\0AUDIOCAPTURE_GUI_PID=92\0\0"
        );
    }

    #[test]
    fn guided_command_line_round_trips_windows_arguments() {
        let worker = Path::new(r"C:\Example Apps\Audio Capture\transcribe.exe");
        let unusual_utf16 = OsString::from_wide(&[0xd800]);
        let arguments = [
            OsStr::new(r"D:\Recorded Audio\one two.wav"),
            OsStr::new(r"D:\Recorded Audio\音声 café & (100%).opus"),
            OsStr::new(""),
            OsStr::new(r"D:\trailing spaces and slashes \\"),
            OsStr::new(r#"quote\\"inside""#),
            unusual_utf16.as_os_str(),
        ];
        let files: Vec<_> = arguments.iter().map(PathBuf::from).collect();
        let command_line = worker_command_line(worker, "--guided --", &files).unwrap();
        let mut count = 0;
        let argv = unsafe { CommandLineToArgvW(command_line.as_ptr(), &mut count) };
        assert!(!argv.is_null());
        // Windows returns one allocation containing count null-terminated arguments.
        let parsed: Vec<_> = unsafe {
            let parsed = std::slice::from_raw_parts(argv, count as usize)
                .iter()
                .map(|arg| OsString::from_wide(PWSTR(*arg).as_wide()))
                .collect();
            LocalFree(argv.cast());
            parsed
        };
        let expected = [worker.as_os_str(), OsStr::new("--guided"), OsStr::new("--")]
            .into_iter()
            .chain(arguments)
            .collect::<Vec<_>>();
        assert_eq!(parsed, expected);
    }

    #[test]
    fn guided_command_line_rejects_an_embedded_null() {
        let invalid = PathBuf::from(OsString::from_wide(&[b'a' as u16, 0, b'b' as u16]));
        let worker = Path::new(r"C:\Example Apps\transcribe.exe");
        let error = worker_command_line(worker, "--guided --", &[invalid]).unwrap_err();
        assert_eq!(error.code(), E_INVALIDARG);
    }
}
