use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;

use crate::native_window::*;
use audio_capture::{capture, prefs, record, sources};

type Hwnd = *mut c_void;
#[link(name = "user32")]
unsafe extern "system" {
    fn SendMessageW(hwnd: Hwnd, message: u32, wparam: usize, lparam: isize) -> isize;
    fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
    fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, count: i32) -> i32;
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
    fn EnableWindow(hwnd: Hwnd, enable: i32) -> i32;
    fn MessageBoxW(hwnd: Hwnd, text: *const u16, caption: *const u16, style: u32) -> i32;
    fn SetTimer(hwnd: Hwnd, id: usize, milliseconds: u32, callback: *const c_void) -> usize;
    fn KillTimer(hwnd: Hwnd, id: usize) -> i32;
}

const CB_ADDSTRING: u32 = 0x0143;
const CB_GETCURSEL: u32 = 0x0147;
const CB_SETCURSEL: u32 = 0x014e;
const CB_RESETCONTENT: u32 = 0x014b;
const LB_ADDSTRING: u32 = 0x0180;
const LB_RESETCONTENT: u32 = 0x0184;
const LB_SETSEL: u32 = 0x0185;
const LB_GETSEL: u32 = 0x0187;
const BM_GETCHECK: u32 = 0x00f0;
const BM_SETCHECK: u32 = 0x00f1;
const CBN_SELCHANGE: u16 = 1;
const LBN_SELCHANGE: u16 = 1;
const BN_CLICKED: u16 = 0;
const SESSION_TIMER: usize = 1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn msg(view: &View, id: i32, message: u32, wparam: usize, lparam: isize) -> isize {
    unsafe { SendMessageW(view.get(id) as Hwnd, message, wparam, lparam) }
}
fn text(view: &View, id: i32) -> String {
    let handle = view.get(id) as Hwnd;
    let len = unsafe { GetWindowTextLengthW(handle) }.max(0) as usize;
    let mut data = vec![0u16; len + 1];
    let n = unsafe { GetWindowTextW(handle, data.as_mut_ptr(), data.len() as i32) }.max(0) as usize;
    String::from_utf16_lossy(&data[..n])
}
fn set_text(view: &View, id: i32, value: &str) {
    unsafe {
        SetWindowTextW(view.get(id) as Hwnd, wide(value).as_ptr());
    }
}
fn selected(view: &View, id: i32) -> usize {
    let result = msg(view, id, CB_GETCURSEL, 0, 0);
    if result < 0 { 0 } else { result as usize }
}
fn combo(view: &View, id: i32, entries: &[String], chosen: usize) {
    msg(view, id, CB_RESETCONTENT, 0, 0);
    for entry in entries {
        msg(view, id, CB_ADDSTRING, 0, wide(entry).as_ptr() as isize);
    }
    msg(
        view,
        id,
        CB_SETCURSEL,
        chosen.min(entries.len().saturating_sub(1)),
        0,
    );
}
fn checked(view: &View, id: i32) -> bool {
    msg(view, id, BM_GETCHECK, 0, 0) == 1
}
fn set_checked(view: &View, id: i32, checked: bool) {
    msg(view, id, BM_SETCHECK, usize::from(checked), 0);
}
fn enabled(view: &View, id: i32, enable: bool) {
    unsafe {
        EnableWindow(view.get(id) as Hwnd, i32::from(enable));
    }
}
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).to_owned()).collect()
}

fn retain_live_selections(
    selected: &mut HashMap<u32, u64>,
    old: &[sources::Process],
    current: &[sources::Process],
    identity: impl Fn(u32) -> Result<u64, String>,
) -> Vec<String> {
    let mut expired = Vec::new();
    selected.retain(|pid, expected| {
        let still_listed = current.iter().any(|process| process.pid == *pid);
        let unchanged = still_listed && identity(*pid).ok() == Some(*expected);
        if !unchanged {
            let name = old
                .iter()
                .find(|process| process.pid == *pid)
                .map(|process| process.name.as_str())
                .unwrap_or("Program");
            expired.push(format!("{name} (PID {pid})"));
        }
        unchanged
    });
    expired.sort();
    expired
}

pub struct Controller {
    settings: prefs::Settings,
    processes: Vec<sources::Process>,
    visible: Vec<usize>,
    selected: HashMap<u32, u64>,
    inputs: Vec<(String, String)>,
    outputs: Vec<(String, String)>,
    missing: Vec<String>,
    session: Option<record::Session>,
    paused: bool,
    status: String,
    settings_load_failed: bool,
}

impl Controller {
    pub fn new() -> Self {
        let (settings, status, settings_load_failed) = match prefs::load() {
            Ok(value) => (value, String::new(), false),
            Err(error) => (
                prefs::Settings::default(),
                format!("{error}. Settings will not be overwritten."),
                true,
            ),
        };
        let missing = settings.process_names.clone();
        let mut this = Self {
            settings,
            processes: Vec::new(),
            visible: Vec::new(),
            selected: HashMap::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            missing,
            session: None,
            paused: false,
            status,
            settings_load_failed,
        };
        this.refresh();
        this.resolve_names();
        this
    }

    fn refresh(&mut self) {
        match sources::processes() {
            Ok(processes) => {
                let expired = retain_live_selections(
                    &mut self.selected,
                    &self.processes,
                    &processes,
                    sources::process_identity,
                );
                self.processes = processes;
                if !expired.is_empty() {
                    self.status = format!(
                        "Selection expired. Select a program again: {}",
                        expired.join(", ")
                    );
                }
            }
            Err(error) => self.status = format!("Could not list programs: {error}"),
        }
        match sources::devices() {
            Ok((inputs, outputs)) => {
                self.inputs = inputs;
                self.outputs = outputs;
            }
            Err(error) => self.status = format!("Could not list audio devices: {error}"),
        }
    }

    fn resolve_names(&mut self) {
        let mut unresolved = Vec::new();
        for name in self.missing.drain(..) {
            let matches: Vec<_> = self
                .processes
                .iter()
                .filter(|p| p.name.eq_ignore_ascii_case(&name))
                .collect();
            if let [process] = matches.as_slice() {
                if let Ok(identity) = sources::process_identity(process.pid) {
                    self.selected.insert(process.pid, identity);
                    continue;
                }
            }
            unresolved.push(name);
        }
        self.missing = unresolved;
    }

    fn save_names(&mut self) {
        // Keep unresolved legacy selections until the user explicitly clears them.
        self.settings.process_names = self
            .processes
            .iter()
            .filter(|p| self.selected.contains_key(&p.pid))
            .map(|p| p.name.clone())
            .chain(self.missing.iter().cloned())
            .collect();
    }

    fn process_list(&mut self, view: &View) {
        self.visible.clear();
        msg(view, PROCESS_LIST, LB_RESETCONTENT, 0, 0);
        for (index, process) in self.processes.iter().enumerate() {
            if self.settings.active_only
                && !process.active
                && !self.selected.contains_key(&process.pid)
            {
                continue;
            }
            let row = format!(
                "{}  |  PID {}  |  {}",
                process.name, process.pid, process.title
            );
            let n = msg(
                view,
                PROCESS_LIST,
                LB_ADDSTRING,
                0,
                wide(&row).as_ptr() as isize,
            );
            if n >= 0 {
                self.visible.push(index);
                if self.selected.contains_key(&process.pid) {
                    msg(view, PROCESS_LIST, LB_SETSEL, 1, n);
                }
            }
        }
    }

    fn update_selection(&mut self, view: &View) {
        for (row, index) in self.visible.iter().copied().enumerate() {
            let process = &self.processes[index];
            let is_selected = msg(view, PROCESS_LIST, LB_GETSEL, row, 0) > 0;
            if is_selected && !self.selected.contains_key(&process.pid) {
                match sources::process_identity(process.pid) {
                    Ok(identity) => {
                        self.selected.insert(process.pid, identity);
                    }
                    Err(error) => {
                        self.status = format!("Cannot select {}: {error}", process.name);
                        msg(view, PROCESS_LIST, LB_SETSEL, 0, row as isize);
                    }
                }
            } else if !is_selected {
                self.selected.remove(&process.pid);
            }
        }
        // Selecting one candidate resolves an ambiguous legacy name explicitly.
        self.missing.retain(|name| {
            !self
                .processes
                .iter()
                .any(|p| self.selected.contains_key(&p.pid) && p.name.eq_ignore_ascii_case(name))
        });
    }

    fn device_combo(view: &View, id: i32, devices: &[(String, String)], saved: &Option<String>) {
        let mut entries = vec!["Off".to_owned()];
        entries.extend(devices.iter().map(|(_, name)| name.clone()));
        let index = saved
            .as_ref()
            .and_then(|id| devices.iter().position(|(key, _)| key == id))
            .map(|n| n + 1);
        if saved.is_some() && index.is_none() {
            entries.push("Unavailable — select a device".into());
            combo(view, id, &entries, entries.len() - 1);
        } else {
            combo(view, id, &entries, index.unwrap_or(0));
        }
    }

    pub fn sync_full(&mut self, view: &View) {
        if self.settings.format == "mp3" && self.settings.channels == 1 {
            self.settings.mp3_kbps = 128;
        }
        let presets = prefs::presets().unwrap_or_default();
        let old_preset = text(view, PRESET);
        combo(view, PRESET, &presets, 0);
        set_text(view, PRESET, &old_preset);
        self.process_list(view);
        set_checked(view, FILTER, self.settings.active_only);
        set_checked(view, SYSTEM, self.settings.system);
        set_checked(view, MONITOR_ONLY, self.settings.monitor_only);
        set_checked(view, SILENCE, self.settings.skip_silence);
        combo(
            view,
            FORMAT,
            &strings(&["WAV", "MP3", "Opus", "FLAC"]),
            ["wav", "mp3", "opus", "flac"]
                .iter()
                .position(|s| *s == self.settings.format)
                .unwrap_or(0),
        );
        self.codec(view);
        let rates = strings(&["Auto (48000 Hz)", "16000 Hz", "44100 Hz", "48000 Hz"]);
        combo(
            view,
            SAMPLE,
            &rates,
            [0, 16000, 44100, 48000]
                .iter()
                .position(|n| *n == self.settings.wav_rate)
                .unwrap_or(0),
        );
        combo(
            view,
            CHANNEL,
            &strings(&["Mono", "Stereo"]),
            usize::from(self.settings.channels == 2),
        );
        Self::device_combo(view, MIC, &self.inputs, &self.settings.mic);
        Self::device_combo(view, MONITOR, &self.outputs, &self.settings.monitor);
        let gain: Vec<_> = (0..=10).map(|n| format!("{}%", n * 10)).collect();
        combo(
            view,
            APP_GAIN,
            &gain,
            (self.settings.app_gain * 10.0).round() as usize,
        );
        combo(
            view,
            MIC_GAIN,
            &gain,
            (self.settings.mic_gain * 10.0).round() as usize,
        );
        let mode = match (self.settings.separate, self.settings.mixed) {
            (false, true) => 1,
            (true, true) => 2,
            _ => 0,
        };
        combo(
            view,
            MODE,
            &strings(&["Separate files", "Combined file", "Both"]),
            mode,
        );
        set_text(view, FOLDER, &self.settings.folder.to_string_lossy());
        self.state_controls(view);
        self.show_status(view);
    }

    fn codec(&self, view: &View) {
        let (label, choices, value): (&str, Vec<u32>, u32) = match self.settings.format.as_str() {
            "opus" => (
                "Opus bitrate (kbps)",
                vec![24, 32, 48, 64, 96, 128, 192, 256],
                self.settings.opus_kbps,
            ),
            "mp3" => (
                "MP3 bitrate (kbps)",
                if self.settings.channels == 1 {
                    vec![128]
                } else {
                    vec![128, 192, 256, 320]
                },
                self.settings.mp3_kbps,
            ),
            "flac" => (
                "FLAC compression level",
                (0..=8).collect(),
                self.settings.flac_level as u32,
            ),
            _ => ("Bitrate", vec![], 0),
        };
        set_text(view, BITRATE_LABEL, label);
        let labels: Vec<String> = choices.iter().map(|v| v.to_string()).collect();
        combo(
            view,
            BITRATE,
            &labels,
            choices.iter().position(|n| *n == value).unwrap_or(0),
        );
        enabled(view, BITRATE, self.session.is_none() && !choices.is_empty());
        enabled(
            view,
            SAMPLE,
            self.session.is_none() && self.settings.format == "wav",
        );
    }

    fn read_controls(&mut self, view: &View) {
        self.settings.active_only = checked(view, FILTER);
        self.settings.system = checked(view, SYSTEM);
        self.settings.monitor_only = checked(view, MONITOR_ONLY);
        self.settings.skip_silence = checked(view, SILENCE);
        self.settings.folder = PathBuf::from(text(view, FOLDER));
        self.settings.wav_rate = [0, 16000, 44100, 48000][selected(view, SAMPLE).min(3)];
        self.settings.channels = if selected(view, CHANNEL) == 0 { 1 } else { 2 };
        self.settings.app_gain = selected(view, APP_GAIN).min(10) as f32 / 10.0;
        self.settings.mic_gain = selected(view, MIC_GAIN).min(10) as f32 / 10.0;
        let mode = selected(view, MODE).min(2);
        self.settings.separate = mode != 1;
        self.settings.mixed = mode != 0;
        let mic = selected(view, MIC);
        if mic <= self.inputs.len() {
            self.settings.mic = mic.checked_sub(1).map(|i| self.inputs[i].0.clone());
        }
        let monitor = selected(view, MONITOR);
        if monitor <= self.outputs.len() {
            self.settings.monitor = monitor.checked_sub(1).map(|i| self.outputs[i].0.clone());
        }
    }

    fn state_controls(&self, view: &View) {
        let running = self.session.is_some();
        for id in [
            PRESET,
            SAVE,
            LOAD,
            DELETE,
            PROCESS_LIST,
            REFRESH,
            FILTER,
            SYSTEM,
            FORMAT,
            CHANNEL,
            MIC,
            MONITOR,
            MONITOR_ONLY,
            APP_GAIN,
            MIC_GAIN,
            MODE,
            SILENCE,
            FOLDER,
            BROWSE,
        ] {
            enabled(view, id, !running);
        }
        enabled(view, START, !running);
        enabled(view, STOP, running);
        enabled(view, PAUSE, running);
        set_text(view, PAUSE, if self.paused { "Resume" } else { "Pause" });
        self.codec(view);
    }

    fn show_status(&self, view: &View) {
        let status = if let Some(session) = &self.session {
            session.status()
        } else {
            self.status.clone()
        };
        let status = if self.missing.is_empty() {
            status
        } else {
            format!(
                "{status}   Unavailable or ambiguous programs: {}",
                self.missing.join(", ")
            )
        };
        set_text(view, STATUS, &status);
    }

    fn start(&mut self) -> Result<(), String> {
        if !self.missing.is_empty() {
            return Err(format!(
                "Select the unresolved programs or clear them when prompted: {}",
                self.missing.join(", ")
            ));
        }
        let mut inputs = Vec::new();
        for p in self
            .processes
            .iter()
            .filter(|p| self.selected.contains_key(&p.pid))
        {
            if sources::process_identity(p.pid).ok() != self.selected.get(&p.pid).copied() {
                return Err(format!("{} exited. Refresh and select it again.", p.name));
            }
            inputs.push((
                p.name.clone(),
                capture::Input::SelectedProcess {
                    pid: p.pid,
                    created: self.selected[&p.pid],
                },
                self.settings.app_gain,
            ));
        }
        if self.settings.system {
            inputs.push((
                "System audio".into(),
                capture::Input::System,
                self.settings.app_gain,
            ));
        }
        if let Some(id) = &self.settings.mic {
            if !self.inputs.iter().any(|(key, _)| key == id) {
                return Err("The selected microphone is unavailable.".into());
            }
            inputs.push((
                "Microphone".into(),
                capture::Input::Mic(id.clone()),
                self.settings.mic_gain,
            ));
        }
        if let Some(id) = &self.settings.monitor {
            if !self.outputs.iter().any(|(key, _)| key == id) {
                return Err("The selected monitor output is unavailable.".into());
            }
        }
        let rate = if self.settings.format == "wav" && self.settings.wav_rate != 0 {
            self.settings.wav_rate
        } else {
            48000
        };
        let config = record::Config {
            inputs,
            folder: self.settings.folder.clone(),
            format: self.settings.format.clone(),
            rate,
            channels: self.settings.channels,
            bitrate: if self.settings.format == "mp3" {
                self.settings.mp3_kbps
            } else {
                self.settings.opus_kbps
            },
            flac_level: self.settings.flac_level,
            separate: self.settings.separate,
            mixed: self.settings.mixed,
            skip_silence: self.settings.skip_silence,
            monitor: self.settings.monitor.clone(),
            monitor_only: self.settings.monitor_only,
        };
        self.save_names();
        if !self.settings_load_failed {
            prefs::save(&self.settings)?;
        }
        self.session = Some(record::Session::start(config)?);
        self.paused = false;
        Ok(())
    }

    pub fn command(&mut self, view: &View, id: i32, notification: u16) {
        if id == PROCESS_LIST && notification == LBN_SELCHANGE {
            self.update_selection(view);
            self.show_status(view);
            return;
        }
        if self.session.is_some() && ![PAUSE, STOP].contains(&id) {
            return;
        }
        if id == FORMAT && notification == CBN_SELCHANGE {
            self.settings.format =
                ["wav", "mp3", "opus", "flac"][selected(view, FORMAT).min(3)].into();
            if self.settings.format == "mp3" && self.settings.channels == 1 {
                self.settings.mp3_kbps = 128;
            }
            self.codec(view);
        } else if id == BITRATE && notification == CBN_SELCHANGE {
            let n = selected(view, BITRATE);
            match self.settings.format.as_str() {
                "opus" => self.settings.opus_kbps = [24, 32, 48, 64, 96, 128, 192, 256][n.min(7)],
                "mp3" => {
                    self.settings.mp3_kbps = if self.settings.channels == 1 {
                        128
                    } else {
                        [128, 192, 256, 320][n.min(3)]
                    }
                }
                "flac" => self.settings.flac_level = n.min(8) as u8,
                _ => {}
            }
        } else if id == CHANNEL && notification == CBN_SELCHANGE {
            self.read_controls(view);
            if self.settings.format == "mp3" && self.settings.channels == 1 {
                self.settings.mp3_kbps = 128;
            }
            self.codec(view);
        } else if id == FILTER && notification == BN_CLICKED {
            self.settings.active_only = checked(view, FILTER);
            self.process_list(view);
        } else if id == REFRESH && notification == BN_CLICKED {
            self.read_controls(view);
            self.refresh();
            self.sync_full(view);
        } else if id == BROWSE && notification == BN_CLICKED {
            if let Some(path) = rfd::FileDialog::new()
                .set_directory(text(view, FOLDER))
                .pick_folder()
            {
                self.settings.folder = path;
                set_text(view, FOLDER, &self.settings.folder.to_string_lossy());
            }
        } else if [SAVE, LOAD, DELETE].contains(&id) && notification == BN_CLICKED {
            self.read_controls(view);
            let name = text(view, PRESET);
            let result = if id == SAVE {
                self.save_names();
                prefs::save_preset(&name, &self.settings).map(|_| "Preset saved".to_owned())
            } else if id == LOAD {
                prefs::load_preset(&name).map(|settings| {
                    self.settings = settings;
                    self.missing = self.settings.process_names.clone();
                    self.selected.clear();
                    self.resolve_names();
                    "Preset loaded".to_owned()
                })
            } else {
                let question = format!("Delete preset \"{name}\"?");
                if unsafe {
                    MessageBoxW(
                        view.hwnd as Hwnd,
                        wide(&question).as_ptr(),
                        wide("Delete preset").as_ptr(),
                        0x24,
                    )
                } != 6
                {
                    return;
                }
                prefs::delete_preset(&name).map(|_| "Preset deleted".to_owned())
            };
            self.status = result.unwrap_or_else(|e| e);
            self.sync_full(view);
            set_text(view, PRESET, &name);
        } else if id == START && notification == BN_CLICKED {
            self.read_controls(view);
            if !self.missing.is_empty() {
                let prompt = format!(
                    "These saved programs are unavailable or ambiguous: {}. Clear them and continue?",
                    self.missing.join(", ")
                );
                if unsafe {
                    MessageBoxW(
                        view.hwnd as Hwnd,
                        wide(&prompt).as_ptr(),
                        wide("Unresolved programs").as_ptr(),
                        0x24,
                    )
                } != 6
                {
                    return;
                }
                self.missing.clear();
            }
            if let Err(error) = self.start() {
                self.status = error;
            }
            if self.session.is_some() {
                unsafe {
                    SetTimer(view.hwnd as Hwnd, SESSION_TIMER, 500, ptr::null());
                }
            }
            self.state_controls(view);
        } else if id == PAUSE && notification == BN_CLICKED {
            if let Some(session) = &self.session {
                self.paused = !self.paused;
                session.pause(self.paused);
            }
            self.state_controls(view);
        } else if id == STOP && notification == BN_CLICKED {
            self.stop(view);
        }
        self.show_status(view);
    }

    fn stop(&mut self, view: &View) {
        unsafe {
            KillTimer(view.hwnd as Hwnd, SESSION_TIMER);
        }
        if let Some(mut session) = self.session.take() {
            self.status = match session.stop() {
                Ok(()) => session.status(),
                Err(error) => error,
            };
        }
        self.paused = false;
        self.state_controls(view);
    }

    pub fn tick(&mut self, view: &View) {
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.is_finished())
        {
            self.stop(view);
        }
        self.show_status(view);
    }

    pub fn shutdown(&mut self, view: &View) -> Result<(), String> {
        let mut errors = Vec::new();
        if let Some(mut session) = self.session.take() {
            unsafe {
                KillTimer(view.hwnd as Hwnd, SESSION_TIMER);
            }
            if let Err(error) = session.stop() {
                errors.push(format!("Recording: {error}"));
            }
        }
        self.paused = false;
        self.read_controls(view);
        self.save_names();
        if !self.settings_load_failed {
            if let Err(error) = prefs::save(&self.settings) {
                errors.push(format!("Settings: {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            let error = errors.join("\n");
            self.status = error.clone();
            self.state_controls(view);
            self.show_status(view);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str) -> sources::Process {
        sources::Process {
            pid,
            name: name.to_owned(),
            title: String::new(),
            active: true,
        }
    }

    #[test]
    fn refresh_keeps_selected_identity_without_matching_same_name_siblings() {
        let old = vec![process(10, "app.exe"), process(20, "app.exe")];
        let current = vec![process(10, "app.exe"), process(20, "app.exe")];
        let mut selected = HashMap::from([(10, 101)]);
        let expired = retain_live_selections(&mut selected, &old, &current, |pid| match pid {
            10 => Ok(101),
            20 => Ok(202),
            _ => Err("Missing process".into()),
        });
        assert_eq!(selected, HashMap::from([(10, 101)]));
        assert!(expired.is_empty());

        // A reused PID and a surviving same-name sibling must not be selected.
        let expired = retain_live_selections(&mut selected, &old, &current, |pid| match pid {
            10 => Ok(999),
            20 => Ok(202),
            _ => Err("Missing process".into()),
        });
        assert!(selected.is_empty());
        assert_eq!(expired, vec!["app.exe (PID 10)"]);
    }

    #[test]
    fn refresh_removes_a_disappeared_pid() {
        let old = vec![process(10, "app.exe"), process(20, "app.exe")];
        let current = vec![process(20, "app.exe")];
        let mut selected = HashMap::from([(10, 101)]);
        let expired = retain_live_selections(&mut selected, &old, &current, |_| {
            panic!("identity query for a disappeared process")
        });
        assert!(selected.is_empty());
        assert_eq!(expired, vec!["app.exe (PID 10)"]);
    }
}
