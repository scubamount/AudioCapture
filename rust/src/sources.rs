use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::thread;

use wasapi::{DeviceEnumerator, Direction, SessionState};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND, LPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};
use windows::core::BOOL;

#[derive(Clone, Debug)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub title: String,
    pub active: bool,
}

/// Identify a process instance without relying on its reusable PID.
pub fn process_identity(pid: u32) -> Result<u64, String> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|error| format!("Cannot inspect process {pid}: {error}"))?;
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let result =
        unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) };
    unsafe {
        let _ = CloseHandle(process);
    }
    result.map_err(|error| format!("Cannot inspect process {pid}: {error}"))?;
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
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

fn active_processes() -> Result<HashSet<u32>, String> {
    in_mta(|| {
        let enumerator = DeviceEnumerator::new().map_err(|e| e.to_string())?;
        let devices = enumerator
            .get_device_collection(Direction::Render)
            .map_err(|e| e.to_string())?;
        let mut active = HashSet::new();
        for index in 0..devices.get_nbr_devices().map_err(|e| e.to_string())? {
            let device = devices
                .get_device_at_index(index)
                .map_err(|e| e.to_string())?;
            let manager = device
                .get_iaudiosessionmanager()
                .map_err(|e| e.to_string())?;
            let sessions = manager
                .get_audiosessionenumerator()
                .map_err(|e| e.to_string())?;
            for session_index in 0..sessions.get_count().map_err(|e| e.to_string())? {
                let session = sessions
                    .get_session(session_index)
                    .map_err(|e| e.to_string())?;
                if matches!(session.get_state(), Ok(SessionState::Active)) {
                    if let Ok(pid) = session.get_process_id() {
                        active.insert(pid);
                    }
                }
            }
        }
        Ok(active)
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
    let active = active_processes()?;
    let mut titles: HashMap<u32, String> = HashMap::new();
    unsafe {
        EnumWindows(Some(collect_window), LPARAM(&mut titles as *mut _ as isize))
            .map_err(|e| format!("Window enumeration failed: {e}"))?;
    }
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
    Ok(processes)
}
