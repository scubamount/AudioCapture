use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;

use crate::native_window::*;
use crate::worker_launch::launch_transcription;
use audio_capture::{capture, prefs, record, sources};
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK};

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
const EM_GETFIRSTVISIBLELINE: u32 = 0x00ce;
const EM_LINESCROLL: u32 = 0x00b6;
const CBN_SELCHANGE: u16 = 1;
const LBN_SELCHANGE: u16 = 1;
const BN_CLICKED: u16 = 0;
const SESSION_TIMER: usize = 1;
const FORMATS: [&str; 5] = ["wav", "mp3", "opus", "flac", "opus-speech"];
const SAMPLE_RATES: [u32; 4] = [0, 16_000, 44_100, 48_000];
const OPUS_BITRATES: [u32; 8] = [24, 32, 48, 64, 96, 128, 192, 256];
const MP3_BITRATES: [u32; 4] = [128, 192, 256, 320];
const FLAC_LEVELS: [u32; 9] = [0, 1, 2, 3, 4, 5, 6, 7, 8];

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
fn set_levels(view: &View, value: &str) {
    let rendered = value.replace("\r\n", "\n").replace('\n', "\r\n");
    if text(view, LEVELS) == rendered {
        return;
    }
    // Restore the top visible source when the live levels change.
    let line = msg(view, LEVELS, EM_GETFIRSTVISIBLELINE, 0, 0);
    set_text(view, LEVELS, &rendered);
    msg(view, LEVELS, EM_LINESCROLL, 0, line);
}
fn gain_index(gain: f32) -> usize {
    (gain.clamp(0.0, 1.0) * 100.0).round() as usize
}
fn gain_value(index: usize) -> f32 {
    index.min(100) as f32 / 100.0
}
fn select_format(settings: &mut prefs::Settings, index: usize) {
    settings.format = FORMATS[index.min(FORMATS.len() - 1)].into();
    if settings.format == "opus-speech" {
        settings.channels = 1;
    }
}
fn selected(view: &View, id: i32) -> usize {
    let result = msg(view, id, CB_GETCURSEL, 0, 0);
    if result < 0 { 0 } else { result as usize }
}
fn combo(view: &View, id: i32, entries: &[impl AsRef<str>], chosen: usize) {
    msg(view, id, CB_RESETCONTENT, 0, 0);
    for entry in entries {
        msg(
            view,
            id,
            CB_ADDSTRING,
            0,
            wide(entry.as_ref()).as_ptr() as isize,
        );
    }
    set_selected(view, id, chosen.min(entries.len().saturating_sub(1)));
}
fn set_selected(view: &View, id: i32, index: usize) {
    msg(view, id, CB_SETCURSEL, index, 0);
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
fn accepts_command(id: i32, session_active: bool) -> bool {
    !session_active || [PAUSE, STOP, MARK].contains(&id)
}

fn transcribe_files(view: &View) {
    let files = rfd::FileDialog::new()
        .set_title("Select files to transcribe")
        .set_directory(text(view, FOLDER))
        .add_filter(
            "Audio and media files",
            audio_capture::TRANSCRIPTION_EXTENSIONS,
        )
        .pick_files();
    let Some(files) = files.filter(|files| !files.is_empty()) else {
        return;
    };
    if let Err(error) = launch_transcription(&files) {
        unsafe {
            MessageBoxW(
                view.hwnd as Hwnd,
                wide(&error).as_ptr(),
                wide("Could not open transcription").as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
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
    discovery_warning: Option<String>,
    marker_count: u64,
    settings_load_failed: bool,
}

impl Controller {
    pub fn new() -> Self {
        let (settings, status, settings_load_failed) = match prefs::load() {
            Ok(value) => (value, String::new(), false),
            Err(error) => (
                prefs::Settings::historical_defaults(),
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
            discovery_warning: None,
            marker_count: 0,
            settings_load_failed,
        };
        this.refresh();
        this.resolve_names();
        this
    }

    fn refresh(&mut self) {
        match sources::processes_with_warnings() {
            Ok((processes, warning)) => {
                self.discovery_warning = warning;
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
            Err(error) => {
                self.discovery_warning = None;
                self.status = format!("Could not list programs: {error}");
            }
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
                && self.discovery_warning.is_none()
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

    pub fn initialize(&mut self, view: &View) {
        let mut profiles: Vec<_> = prefs::ENCODING_PROFILES.iter().map(|p| p.label).collect();
        profiles.push("Custom");
        combo(view, PROFILE, &profiles, 0);
        combo(
            view,
            FORMAT,
            &[
                "WAV",
                "MP3",
                "Opus",
                "FLAC",
                "Opus speech DTX (experimental)",
            ],
            0,
        );
        combo(
            view,
            SAMPLE,
            &["Auto (48000 Hz)", "16000 Hz", "44100 Hz", "48000 Hz"],
            0,
        );
        combo(view, CHANNEL, &["Mono", "Stereo"], 0);
        self.sync_full(view);
    }

    fn sync_full(&mut self, view: &View) {
        let presets = prefs::presets().unwrap_or_default();
        let old_preset = text(view, PRESET);
        combo(view, PRESET, &presets, 0);
        set_text(view, PRESET, &old_preset);
        self.process_list(view);
        set_checked(view, FILTER, self.settings.active_only);
        set_checked(view, SYSTEM, self.settings.system);
        set_checked(view, MONITOR_ONLY, self.settings.monitor_only);
        set_checked(view, SILENCE, self.settings.skip_silence);
        self.sync_encoding(view);
        Self::device_combo(view, MIC, &self.inputs, &self.settings.mic);
        Self::device_combo(view, MONITOR, &self.outputs, &self.settings.monitor);
        let gain: Vec<_> = (0..=100).map(|n| format!("{n}%")).collect();
        combo(view, APP_GAIN, &gain, gain_index(self.settings.app_gain));
        combo(view, MIC_GAIN, &gain, gain_index(self.settings.mic_gain));
        let mode = match (self.settings.separate, self.settings.mixed) {
            (false, true) => 1,
            (true, true) => 2,
            _ => 0,
        };
        combo(
            view,
            MODE,
            &["Separate files", "Combined file", "Both"],
            mode,
        );
        set_text(view, FOLDER, &self.settings.folder.to_string_lossy());
        self.state_controls(view);
        self.show_status(view);
    }

    fn sync_encoding(&mut self, view: &View) {
        if self.settings.format == "mp3" && self.settings.channels == 1 {
            self.settings.mp3_kbps = 128;
        }
        set_selected(
            view,
            FORMAT,
            FORMATS
                .iter()
                .position(|s| *s == self.settings.format)
                .unwrap_or(0),
        );
        let rate = if self.settings.format == "wav" {
            self.settings.wav_rate
        } else {
            self.settings.encoding_rate()
        };
        set_selected(
            view,
            SAMPLE,
            SAMPLE_RATES.iter().position(|n| *n == rate).unwrap_or(0),
        );
        set_selected(view, CHANNEL, usize::from(self.settings.channels == 2));
        self.sync_bitrate(view);
        self.sync_profile(view);
        self.encoding_enabled(view);
    }

    fn sync_profile(&self, view: &View) {
        let matched = prefs::matched_encoding_profile(&self.settings);
        let index = matched
            .and_then(|matched| prefs::ENCODING_PROFILES.iter().position(|p| p == matched))
            .unwrap_or(prefs::ENCODING_PROFILES.len());
        set_selected(view, PROFILE, index);
        set_text(
            view,
            PROFILE_HELP,
            matched.map_or("Current settings do not match a built-in profile.", |p| {
                p.summary
            }),
        );
    }

    fn sync_bitrate(&self, view: &View) {
        let (label, choices, value): (&str, &[u32], u32) = match self.settings.format.as_str() {
            "opus" | "opus-speech" => (
                "Opus bitrate (kbps)",
                &OPUS_BITRATES,
                self.settings.opus_kbps,
            ),
            "mp3" => (
                "MP3 bitrate (kbps)",
                if self.settings.channels == 1 {
                    &MP3_BITRATES[..1]
                } else {
                    &MP3_BITRATES
                },
                self.settings.mp3_kbps,
            ),
            "flac" => (
                "FLAC compression level",
                &FLAC_LEVELS,
                self.settings.flac_level.into(),
            ),
            _ => {
                let pcm_kbps =
                    self.settings.encoding_rate() * u32::from(self.settings.channels) * 16;
                set_text(view, BITRATE_LABEL, "PCM bitrate (kbps)");
                combo(
                    view,
                    BITRATE,
                    &[(f64::from(pcm_kbps) / 1000.0).to_string()],
                    0,
                );
                return;
            }
        };
        set_text(view, BITRATE_LABEL, label);
        let labels: Vec<_> = choices.iter().map(u32::to_string).collect();
        combo(
            view,
            BITRATE,
            &labels,
            choices.iter().position(|n| *n == value).unwrap_or(0),
        );
    }

    fn encoding_enabled(&self, view: &View) {
        let idle = self.session.is_none();
        let wav = self.settings.format == "wav";
        enabled(view, PROFILE, idle);
        enabled(view, FORMAT, idle);
        enabled(view, BITRATE, idle && !wav);
        enabled(view, SAMPLE, idle && wav);
        enabled(view, CHANNEL, idle && self.settings.format != "opus-speech");
    }

    fn read_controls(&mut self, view: &View) {
        self.settings.active_only = checked(view, FILTER);
        self.settings.system = checked(view, SYSTEM);
        self.settings.monitor_only = checked(view, MONITOR_ONLY);
        self.settings.skip_silence = checked(view, SILENCE);
        self.settings.folder = PathBuf::from(text(view, FOLDER));
        if self.settings.format == "wav" {
            self.settings.wav_rate =
                SAMPLE_RATES[selected(view, SAMPLE).min(SAMPLE_RATES.len() - 1)];
        }
        self.settings.channels = if selected(view, CHANNEL) == 0 { 1 } else { 2 };
        self.settings.app_gain = gain_value(selected(view, APP_GAIN));
        self.settings.mic_gain = gain_value(selected(view, MIC_GAIN));
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
        enabled(view, MARK, running && !self.settings.monitor_only);
        enabled(
            view,
            TRANSCRIBE_FILES,
            accepts_command(TRANSCRIBE_FILES, running),
        );
        set_text(view, PAUSE, if self.paused { "Resume" } else { "Pause" });
        self.encoding_enabled(view);
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
        let status = if self.session.is_some() && self.marker_count > 0 {
            format!("{status}   Marker {} queued.", self.marker_count)
        } else {
            status
        };
        let status = if let Some(warning) = &self.discovery_warning {
            format!("{status}   Audio activity list incomplete: {warning}. Showing all programs.")
        } else {
            status
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
        let config = record::Config {
            inputs,
            folder: self.settings.folder.clone(),
            format: self.settings.format.clone(),
            rate: self.settings.encoding_rate(),
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
        self.marker_count = 0;
        self.paused = false;
        Ok(())
    }

    pub fn command(&mut self, view: &View, id: i32, notification: u16) {
        if id == PROCESS_LIST && notification == LBN_SELCHANGE {
            self.update_selection(view);
            self.show_status(view);
            return;
        }
        if !accepts_command(id, self.session.is_some()) {
            return;
        }
        if id == PROFILE && notification == CBN_SELCHANGE {
            let index = msg(view, PROFILE, CB_GETCURSEL, 0, 0);
            if let Some(profile) = prefs::ENCODING_PROFILES.get(index as usize) {
                profile.apply(&mut self.settings);
                self.sync_encoding(view);
            } else {
                // Custom describes an unmatched encoding and does not apply settings.
                self.sync_profile(view);
            }
        } else if id == FORMAT && notification == CBN_SELCHANGE {
            select_format(&mut self.settings, selected(view, FORMAT));
            self.sync_encoding(view);
        } else if id == BITRATE && notification == CBN_SELCHANGE {
            let n = selected(view, BITRATE);
            match self.settings.format.as_str() {
                "opus" | "opus-speech" => {
                    self.settings.opus_kbps = OPUS_BITRATES[n.min(OPUS_BITRATES.len() - 1)]
                }
                "mp3" => {
                    self.settings.mp3_kbps = if self.settings.channels == 1 {
                        MP3_BITRATES[0]
                    } else {
                        MP3_BITRATES[n.min(MP3_BITRATES.len() - 1)]
                    }
                }
                "flac" => {
                    self.settings.flac_level = FLAC_LEVELS[n.min(FLAC_LEVELS.len() - 1)] as u8
                }
                _ => {}
            }
            self.sync_profile(view);
        } else if id == SAMPLE && notification == CBN_SELCHANGE && self.settings.format == "wav" {
            self.settings.wav_rate =
                SAMPLE_RATES[selected(view, SAMPLE).min(SAMPLE_RATES.len() - 1)];
            self.sync_bitrate(view);
            self.sync_profile(view);
        } else if id == CHANNEL && notification == CBN_SELCHANGE {
            self.settings.channels = if selected(view, CHANNEL) == 0 { 1 } else { 2 };
            self.sync_encoding(view);
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
        } else if id == TRANSCRIBE_FILES && notification == BN_CLICKED {
            transcribe_files(view);
            return;
        } else if [SAVE, LOAD, DELETE].contains(&id) && notification == BN_CLICKED {
            self.read_controls(view);
            let name = text(view, PRESET);
            let result = if id == SAVE {
                self.save_names();
                prefs::save_preset(&name, &self.settings).map(|_| "Setup saved".to_owned())
            } else if id == LOAD {
                prefs::load_preset(&name).map(|settings| {
                    self.settings = settings;
                    self.missing = self.settings.process_names.clone();
                    self.selected.clear();
                    self.resolve_names();
                    "Setup loaded".to_owned()
                })
            } else {
                let question = format!("Delete setup \"{name}\"?");
                if unsafe {
                    MessageBoxW(
                        view.hwnd as Hwnd,
                        wide(&question).as_ptr(),
                        wide("Delete setup").as_ptr(),
                        0x24,
                    )
                } != 6
                {
                    return;
                }
                prefs::delete_preset(&name).map(|_| "Setup deleted".to_owned())
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
                    SetTimer(view.hwnd as Hwnd, SESSION_TIMER, 250, ptr::null());
                }
            }
            self.state_controls(view);
        } else if id == PAUSE && notification == BN_CLICKED {
            if let Some(session) = &self.session {
                self.paused = !self.paused;
                session.pause(self.paused);
            }
            self.state_controls(view);
        } else if id == MARK && notification == BN_CLICKED {
            if let Some(session) = &self.session {
                let number = self.marker_count + 1;
                match session.mark(&format!("Marker {number}")) {
                    Ok(()) => self.marker_count = number,
                    Err(error) => unsafe {
                        MessageBoxW(
                            view.hwnd as Hwnd,
                            wide(&error).as_ptr(),
                            wide("Could not add marker").as_ptr(),
                            0x10,
                        );
                    },
                }
            }
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
        set_levels(view, "Source levels appear while recording.");
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
        if let Some(session) = &self.session {
            set_levels(view, &session.levels());
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

    #[test]
    fn transcription_is_available_only_without_an_active_session() {
        assert!(accepts_command(TRANSCRIBE_FILES, false));
        assert!(!accepts_command(TRANSCRIBE_FILES, true));
        for id in [PAUSE, STOP, MARK] {
            assert!(accepts_command(id, true));
        }
    }

    fn process(pid: u32, name: &str) -> sources::Process {
        sources::Process {
            pid,
            name: name.to_owned(),
            title: String::new(),
            active: true,
        }
    }

    #[test]
    fn gain_control_preserves_legacy_percentages() {
        for percent in 0..=100 {
            let saved = percent as f32 / 100.0;
            let read_back = gain_value(gain_index(saved));
            assert!((saved - read_back).abs() < f32::EPSILON);
        }
        assert_eq!(gain_index(0.34), 34);
        assert!((gain_value(34) - 0.34).abs() < f32::EPSILON);
    }

    #[test]
    fn speech_format_selects_mono_and_preserves_bitrate() {
        let mut settings = prefs::Settings {
            format: "opus".into(),
            channels: 2,
            opus_kbps: 32,
            ..Default::default()
        };
        select_format(&mut settings, 4);
        assert_eq!(settings.format, "opus-speech");
        assert_eq!(settings.channels, 1);
        assert_eq!(settings.opus_kbps, 32);
        select_format(&mut settings, 2);
        assert_eq!(settings.format, "opus");
        assert_eq!(settings.opus_kbps, 32);
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
