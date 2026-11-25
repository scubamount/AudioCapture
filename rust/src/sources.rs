use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::{Error, Read};
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use wasapi::{DeviceEnumerator, Direction, SessionState};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, FILETIME, HANDLE, HWND, LPARAM,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};
use windows::core::{BOOL, PWSTR};
use windows_sys::Win32::Security::Cryptography::{
    CALG_SHA_256, CRYPT_VERIFYCONTEXT, CryptAcquireContextW, CryptCreateHash, CryptDestroyHash,
    CryptGetHashParam, CryptHashData, CryptReleaseContext, HP_HASHVAL, PROV_RSA_AES,
};

#[derive(Clone, Debug)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub title: String,
    pub active: bool,
}

/// Identify a process instance without relying on its reusable PID.
pub fn process_identity(pid: u32) -> Result<u64, String> {
    with_process(pid, creation_time)
}

fn with_process<T>(
    pid: u32,
    inspect: impl FnOnce(HANDLE) -> Result<T, String>,
) -> Result<T, String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|error| format!("Cannot inspect process {pid}: {error}"))?;
    let result = inspect(handle);
    unsafe {
        let _ = CloseHandle(handle);
    }
    result
}

fn creation_time(process: HANDLE) -> Result<u64, String> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(|error| format!("Cannot inspect process creation time: {error}"))?;
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// Read the creation time and image path using the same process handle.
pub(crate) fn process_image(pid: u32) -> Result<(u64, PathBuf), String> {
    with_process(pid, |process| {
        let created = creation_time(process)?;
        let mut path = vec![0u16; 512];
        loop {
            let mut size = path.len() as u32;
            let result = unsafe {
                QueryFullProcessImageNameW(
                    process,
                    PROCESS_NAME_FORMAT(0),
                    PWSTR(path.as_mut_ptr()),
                    &mut size,
                )
            };
            match result {
                Ok(()) => {
                    let path = OsString::from_wide(&path[..size as usize]);
                    return Ok((created, path.into()));
                }
                Err(error)
                    if error.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult()
                        && path.len() < 32_768 =>
                {
                    path.resize((path.len() * 2).min(32_768), 0);
                }
                Err(error) => {
                    return Err(format!("Cannot identify the image for PID {pid}: {error}"));
                }
            }
        }
    })
}

/// Stream SHA-256 using Windows' existing cryptography provider.
pub fn sha256(mut input: impl Read) -> Result<[u8; 32], String> {
    struct Provider(usize);
    impl Drop for Provider {
        fn drop(&mut self) {
            unsafe { CryptReleaseContext(self.0, 0) };
        }
    }
    struct Hash(usize);
    impl Drop for Hash {
        fn drop(&mut self) {
            unsafe { CryptDestroyHash(self.0) };
        }
    }

    let mut provider = 0;
    if unsafe {
        CryptAcquireContextW(
            &mut provider,
            std::ptr::null(),
            std::ptr::null(),
            PROV_RSA_AES,
            CRYPT_VERIFYCONTEXT,
        )
    } == 0
    {
        return Err(format!(
            "Cannot open SHA-256 provider: {}",
            Error::last_os_error()
        ));
    }
    let provider = Provider(provider);
    let mut hash = 0;
    if unsafe { CryptCreateHash(provider.0, CALG_SHA_256, 0, 0, &mut hash) } == 0 {
        return Err(format!(
            "Cannot create SHA-256 hash: {}",
            Error::last_os_error()
        ));
    }
    let hash = Hash(hash);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|error| format!("Cannot read input for SHA-256: {error}"))?;
        if count == 0 {
            break;
        }
        if unsafe { CryptHashData(hash.0, buffer.as_ptr(), count as u32, 0) } == 0 {
            return Err(format!("Cannot hash input: {}", Error::last_os_error()));
        }
    }
    let mut bytes = [0u8; 32];
    let mut length = bytes.len() as u32;
    if unsafe { CryptGetHashParam(hash.0, HP_HASHVAL, bytes.as_mut_ptr(), &mut length, 0) } == 0
        || length as usize != bytes.len()
    {
        return Err(format!(
            "Cannot read SHA-256 result: {}",
            Error::last_os_error()
        ));
    }
    Ok(bytes)
}

// WASAPI objects stay on the fresh thread on which COM is initialized.
fn in_mta<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    thread::spawn(move || {
        wasapi::initialize_mta()
            .ok()
            .map_err(|e| format!("Audio initialization failed: {e}"))?;
        let result = operation();
        wasapi::deinitialize();
        result
    })
    .join()
    .map_err(|_| "Audio enumeration thread failed".to_string())?
}

pub fn devices() -> Result<(Vec<(String, String)>, Vec<(String, String)>), String> {
    in_mta(|| {
        let enumerator = DeviceEnumerator::new().map_err(|e| e.to_string())?;
        fn collect(
            enumerator: &DeviceEnumerator,
            direction: Direction,
        ) -> Result<Vec<(String, String)>, String> {
            let collection = enumerator
                .get_device_collection(direction)
                .map_err(|e| e.to_string())?;
            let mut devices = Vec::new();
            for index in 0..collection.get_nbr_devices().map_err(|e| e.to_string())? {
                let device = collection
                    .get_device_at_index(index)
                    .map_err(|e| e.to_string())?;
                devices.push((
                    device.get_id().map_err(|e| e.to_string())?,
                    device.get_friendlyname().map_err(|e| e.to_string())?,
                ));
            }
            Ok(devices)
        }
        Ok((
            collect(&enumerator, Direction::Capture)?,
            collect(&enumerator, Direction::Render)?,
        ))
    })
}

fn process_parents() -> Result<HashMap<u32, u32>, String> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|error| format!("Process enumeration failed: {error}"))?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut parents = HashMap::new();
    unsafe {
        let mut result = Process32FirstW(snapshot, &mut entry);
        if let Err(error) = &result {
            let _ = CloseHandle(snapshot);
            return Err(format!("Process enumeration failed: {error}"));
        }
        while result.is_ok() {
            parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            result = Process32NextW(snapshot, &mut entry);
        }
        let _ = CloseHandle(snapshot);
    }
    Ok(parents)
}

fn overlapping_processes(
    selected: &[(String, u32)],
    parents: &HashMap<u32, u32>,
) -> Option<(String, String)> {
    for (index, (name, pid)) in selected.iter().enumerate() {
        for (other_name, other_pid) in selected.iter().skip(index + 1) {
            for (ancestor, descendant) in [(*pid, *other_pid), (*other_pid, *pid)] {
                let mut current = descendant;
                let mut visited = HashSet::new();
                while visited.insert(current) {
                    if current == ancestor {
                        return Some((name.clone(), other_name.clone()));
                    }
                    let Some(&parent) = parents.get(&current) else {
                        break;
                    };
                    if parent == 0 || parent == current {
                        break;
                    }
                    current = parent;
                }
            }
        }
    }
    None
}

/// Resolve the render endpoint and reject known duplicate or feedback routes.
///
/// This runs before output files are opened. The caller can select overlapping
/// tracks in separate-only mode, but must not combine them or monitor them.
pub fn validate_routes(
    inputs: &mut [(String, crate::capture::Input, f32)],
    mixes: bool,
    monitor_id: Option<&str>,
) -> Result<(), String> {
    use crate::capture::Input;

    let selected: Vec<(String, u32)> = inputs
        .iter()
        .filter_map(|(name, input, _)| match input {
            Input::Process(pid) | Input::SelectedProcess { pid, .. } => Some((name.clone(), *pid)),
            _ => None,
        })
        .collect();
    let has_system = inputs
        .iter()
        .any(|(_, input, _)| matches!(input, Input::System | Input::SystemEndpoint(_)));

    if mixes && has_system && !selected.is_empty() {
        return Err(format!(
            "System audio can include {}. Choose separate files or remove one source before mixing or monitoring.",
            selected[0].0
        ));
    }
    if mixes && selected.len() > 1 {
        let parents = process_parents()?;
        for (name, pid) in &selected {
            if !parents.contains_key(pid) {
                return Err(format!(
                    "Cannot determine the process tree for {name}. Refresh and select it again."
                ));
            }
        }
        if let Some((first, second)) = overlapping_processes(&selected, &parents) {
            return Err(format!(
                "{first} and {second} have overlapping process trees. Choose separate files or remove one source."
            ));
        }
    }

    let needs_default = inputs
        .iter()
        .any(|(_, input, _)| matches!(input, Input::System));
    let endpoint = if needs_default {
        Some(in_mta(|| {
            DeviceEnumerator::new()
                .and_then(|enumerator| enumerator.get_default_device(Direction::Render))
                .and_then(|device| device.get_id())
                .map_err(|error| format!("Cannot identify the system playback device: {error}"))
        })?)
    } else {
        None
    };
    for (_, input, _) in inputs.iter() {
        let system_id = match input {
            Input::System => endpoint.as_deref(),
            Input::SystemEndpoint(id) => Some(id.as_str()),
            _ => None,
        };
        if system_id.is_some_and(str::is_empty) {
            return Err("Cannot identify the system playback device.".into());
        }
        if let (Some(system_id), Some(monitor_id)) = (system_id, monitor_id) {
            if monitor_id.is_empty() {
                return Err("Cannot identify the monitor playback device.".into());
            }
            if system_id.eq_ignore_ascii_case(monitor_id) {
                return Err(
                    "System audio cannot be monitored through the same playback device.".into(),
                );
            }
        }
    }
    if let Some(endpoint) = endpoint {
        for (_, input, _) in inputs.iter_mut() {
            if matches!(input, Input::System) {
                *input = Input::SystemEndpoint(endpoint.clone());
            }
        }
    }
    Ok(())
}

fn active_processes() -> Result<(HashSet<u32>, bool), String> {
    in_mta(|| {
        let enumerator = DeviceEnumerator::new().map_err(|e| e.to_string())?;
        let devices = enumerator
            .get_device_collection(Direction::Render)
            .map_err(|e| e.to_string())?;
        let mut active = HashSet::new();
        let mut incomplete = false;
        for index in 0..devices.get_nbr_devices().map_err(|e| e.to_string())? {
            let sessions = match devices
                .get_device_at_index(index)
                .and_then(|device| device.get_iaudiosessionmanager())
                .and_then(|manager| manager.get_audiosessionenumerator())
            {
                Ok(sessions) => sessions,
                Err(_) => {
                    incomplete = true;
                    continue;
                }
            };
            let count = match sessions.get_count() {
                Ok(count) => count,
                Err(_) => {
                    incomplete = true;
                    continue;
                }
            };
            for session_index in 0..count {
                let session = match sessions.get_session(session_index) {
                    Ok(session) => session,
                    Err(_) => {
                        incomplete = true;
                        continue;
                    }
                };
                match session.get_state() {
                    Ok(SessionState::Active) => match session.get_process_id() {
                        Ok(pid) => {
                            active.insert(pid);
                        }
                        Err(_) => incomplete = true,
                    },
                    Ok(_) => {}
                    Err(_) => incomplete = true,
                }
            }
        }
        Ok((active, incomplete))
    })
}

unsafe extern "system" fn collect_window(hwnd: HWND, data: LPARAM) -> BOOL {
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return BOOL(1);
    }
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return BOOL(1);
    }
    let mut title = vec![0u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(hwnd, &mut title) };
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid != 0 && copied > 0 {
        let titles = unsafe { &mut *(data.0 as *mut HashMap<u32, String>) };
        titles
            .entry(pid)
            .or_insert_with(|| String::from_utf16_lossy(&title[..copied as usize]));
    }
    BOOL(1)
}

pub fn processes() -> Result<Vec<Process>, String> {
    processes_with_warnings().map(|(processes, _)| processes)
}

fn activity_with_warning(
    result: Result<(HashSet<u32>, bool), String>,
) -> (HashSet<u32>, Option<String>) {
    match result {
        Ok((active, false)) => (active, None),
        Ok((active, true)) => (
            active,
            Some("Audio activity information is incomplete".to_string()),
        ),
        Err(error) => (
            HashSet::new(),
            Some(format!(
                "Audio activity information is unavailable: {error}"
            )),
        ),
    }
}

/// Enumerate processes even when some audio-session information is unavailable.
pub fn processes_with_warnings() -> Result<(Vec<Process>, Option<String>), String> {
    let snapshot = process_snapshot()?;
    Ok((snapshot.processes, snapshot.warning))
}

pub(crate) struct ProcessSnapshot {
    pub processes: Vec<Process>,
    pub parents: HashMap<u32, u32>,
    pub warning: Option<String>,
    pub taken_at: u64,
}

pub(crate) fn process_snapshot() -> Result<ProcessSnapshot, String> {
    let (active, warning) = activity_with_warning(active_processes());
    let mut titles: HashMap<u32, String> = HashMap::new();
    unsafe {
        EnumWindows(Some(collect_window), LPARAM(&mut titles as *mut _ as isize))
            .map_err(|e| format!("Window enumeration failed: {e}"))?;
    }
    // Process timestamps use 100 ns ticks since 1601. Keep the time of the
    // snapshot so callers can reject a PID reused before its later inspection.
    let unix_ticks = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| (elapsed.as_nanos() / 100) as u64);
    let taken_at = unix_ticks.saturating_add(116_444_736_000_000_000);
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|e| format!("Process enumeration failed: {e}"))?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut processes = Vec::new();
    let mut parents = HashMap::new();
    unsafe {
        let mut result = Process32FirstW(snapshot, &mut entry);
        if let Err(error) = &result {
            let _ = CloseHandle(snapshot);
            return Err(format!("Process enumeration failed: {error}"));
        }
        while result.is_ok() {
            let pid = entry.th32ProcessID;
            parents.insert(pid, entry.th32ParentProcessID);
            let end = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            if pid != 0 {
                processes.push(Process {
                    pid,
                    name: String::from_utf16_lossy(&entry.szExeFile[..end]),
                    title: titles.remove(&pid).unwrap_or_default(),
                    active: active.contains(&pid),
                });
            }
            result = Process32NextW(snapshot, &mut entry);
        }
        let _ = CloseHandle(snapshot);
    }
    let mut active_with_parents = active.clone();
    for pid in active {
        let mut current = pid;
        for _ in 0..64 {
            let Some(&parent) = parents.get(&current) else {
                break;
            };
            if parent == 0 || parent == current || !active_with_parents.insert(parent) {
                break;
            }
            current = parent;
        }
    }
    for process in &mut processes {
        process.active = active_with_parents.contains(&process.pid);
    }
    processes.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.pid.cmp(&b.pid))
    });
    Ok(ProcessSnapshot {
        processes,
        parents,
        warning,
        taken_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::Input;

    #[test]
    fn selected_process_tree_overlap_is_detected() {
        let parents = HashMap::from([(3, 2), (2, 1), (8, 7), (7, 0)]);
        let selected = vec![("root".into(), 1), ("grandchild".into(), 3)];
        assert_eq!(
            overlapping_processes(&selected, &parents),
            Some(("root".into(), "grandchild".into()))
        );
        let siblings = vec![("first".into(), 2), ("other".into(), 8)];
        assert_eq!(overlapping_processes(&siblings, &parents), None);
        let duplicate = vec![("first".into(), 2), ("again".into(), 2)];
        assert!(overlapping_processes(&duplicate, &parents).is_some());
    }

    #[test]
    fn process_parent_cycles_do_not_loop() {
        let parents = HashMap::from([(2, 3), (3, 2)]);
        assert!(overlapping_processes(&[("a".into(), 1), ("b".into(), 2)], &parents).is_none());
    }

    #[test]
    fn system_and_process_mix_is_rejected_before_device_enumeration() {
        let mut inputs = [
            ("System".into(), Input::System, 1.0),
            ("App".into(), Input::Process(42), 1.0),
        ];
        assert!(
            validate_routes(&mut inputs, true, None)
                .unwrap_err()
                .contains("App")
        );
        assert!(matches!(inputs[0].1, Input::System));
    }

    #[test]
    fn same_endpoint_monitor_is_rejected_but_separate_tracks_are_allowed() {
        let mut inputs = [
            (
                "System".into(),
                Input::SystemEndpoint("DEVICE-ID".into()),
                1.0,
            ),
            ("App".into(), Input::Process(42), 1.0),
        ];
        assert!(validate_routes(&mut inputs, false, None).is_ok());
        assert!(validate_routes(&mut inputs, false, Some("device-id")).is_err());
        assert!(validate_routes(&mut inputs, false, Some("other-device")).is_ok());
    }

    #[test]
    fn activity_failures_are_reported_without_discarding_usable_sessions() {
        let (active, warning) = activity_with_warning(Ok((HashSet::from([123]), true)));
        assert!(active.contains(&123));
        assert!(warning.unwrap().contains("incomplete"));
        let (active, warning) = activity_with_warning(Err("endpoint vanished".into()));
        assert!(active.is_empty());
        assert!(warning.unwrap().contains("endpoint vanished"));
    }
}
