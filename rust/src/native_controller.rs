mod live;
mod recents;

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;

use crate::native_live::{Options as LiveOptions, Status as LiveStatus, Worker as LiveWorker};
use crate::native_window::*;
use crate::worker_launch::{launch_batch_manager, launch_transcription};
use audio_capture::{capture, prefs, record, sources};
use std::time::Instant;
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK};

type Hwnd = *mut c_void;
#[link(name = "user32")]
unsafe extern "system" {
    fn SendMessageW(hwnd: Hwnd, message: u32, wparam: usize, lparam: isize) -> isize;
    fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
    fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, count: i32) -> i32;
    fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32;
    fn EnableWindow(hwnd: Hwnd, enable: i32) -> i32;
    fn IsWindowEnabled(hwnd: Hwnd) -> i32;
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
fn clock(seconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    )
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
        let hwnd = view.get(id) as Hwnd;
        if (IsWindowEnabled(hwnd) != 0) != enable {
            EnableWindow(hwnd, i32::from(enable));
        }
    }
}
fn accepts_command(id: i32, scope_locked: bool) -> bool {
    !scope_locked
        || [
            START,
            PAUSE,
            STOP,
            MARK,
            LIVE_TOGGLE,
            STOP_BOTH,
            LIVE_SPEAKERS,
            LIVE_CANCEL_PENDING,
            TRANSCRIBE_FILES,
            BATCH_MANAGE,
            LIVE_OPEN_TEXT,
            LIVE_OPEN_FOLDER,
            LIVE_BROWSE,
            LOAD_LIVE_CONFIG,
            SAVE_LIVE_CONFIG,
        ]
        .contains(&id)
}

fn encoding_label(settings: &prefs::Settings) -> String {
    let channels = if settings.channels == 1 {
        "mono"
    } else {
        "stereo"
    };
    if settings.format == "wav" {
        let auto = if settings.wav_rate == 0 {
            " (Auto)"
        } else {
            ""
        };
        return format!("WAV {} Hz {channels}{auto}", settings.encoding_rate());
    }
    if let Some(profile) = prefs::matched_encoding_profile(settings) {
        return profile.label.to_owned();
    }
    match settings.format.as_str() {
        "opus" => format!("Opus {} kbps {channels}", settings.opus_kbps),
        "opus-speech" => format!("Opus speech DTX {} kbps {channels}", settings.opus_kbps),
        "mp3" => format!("MP3 {} kbps {channels}", settings.mp3_kbps),
        "flac" => format!("FLAC level {} {channels}", settings.flac_level),
        format => format!("{format} {channels}"),
    }
}

fn set_changed(view: &View, id: i32, value: &str) {
    if text(view, id) != value {
        set_text(view, id, value);
    }
}

fn transcribe_files(view: &View) {
    let files = rfd::FileDialog::new()
        .set_title("Select files to transcribe")
        .set_directory(text(view, FOLDER))
        .add_filter(
            "Audio and media files",
            audio_capture::TRANSCRIPTION_EXTENSIONS,
        )
        .add_filter("Finalized recording sessions", &["jsonl"])
        .pick_files();
    let Some(files) = files.filter(|files| !files.is_empty()) else {
        return;
    };
    let config = live::batch_transcription_config(view);
    if let Err(error) = launch_transcription(&files, config.as_ref()) {
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
    encoding_summary: String,
    processes: Vec<sources::Process>,
    visible: Vec<usize>,
    selected: HashMap<u32, u64>,
    inputs: Vec<(String, String)>,
    outputs: Vec<(String, String)>,
    missing: Vec<String>,
    session: Option<record::Session>,
    retiring: Vec<record::Session>,
    recents: recents::Recents,
    live: Option<LiveWorker>,
    pending_config: Option<record::Config>,
    finishing: Vec<LiveWorker>,
    last_live: LiveStatus,
    live_note: String,
    recording_started: Option<Instant>,
    recording_warning: Option<String>,
    closing: bool,
    close_errors: Vec<String>,
    close_recovery: Option<LiveStatus>,
    timer_on: Cell<bool>,
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
        Self::from_settings(settings, status, settings_load_failed)
    }

    fn from_settings(
        settings: prefs::Settings,
        status: String,
        settings_load_failed: bool,
    ) -> Self {
        let missing = settings.process_names.clone();
        Self {
            encoding_summary: encoding_label(&settings),
            settings,
            processes: Vec::new(),
            visible: Vec::new(),
            selected: HashMap::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            missing,
            session: None,
            retiring: Vec::new(),
            recents: recents::Recents::default(),
            live: None,
            pending_config: None,
            finishing: Vec::new(),
            last_live: LiveStatus::default(),
            live_note: "Ready".into(),
            recording_started: None,
            recording_warning: None,
            closing: false,
            close_errors: Vec::new(),
            close_recovery: None,
            timer_on: Cell::new(false),
            paused: false,
            status,
            discovery_warning: None,
            marker_count: 0,
            settings_load_failed,
        }
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
        self.settings.remembered_program = self.recent_source_marker();
    }

    fn process_list(&mut self, view: &View) {
        let mut widest = 0usize;
        self.visible.clear();
        msg(view, PROCESS_LIST, LB_RESETCONTENT, 0, 0);
        for (index, process) in self.processes.iter().enumerate() {
            if self.settings.active_only
                && self.discovery_warning.is_none()
                && !process.active
                && !self.selected.contains_key(&process.pid)
                && !self
                    .missing
                    .iter()
                    .any(|name| process.name.eq_ignore_ascii_case(name))
            {
                continue;
            }
            let path = self
                .recent_image(process.pid)
                .map(|path| format!("  |  {}", path.display()))
                .unwrap_or_default();
            let row = format!(
                "{}  |  PID {}  |  {}{path}",
                process.name, process.pid, process.title
            );
            widest = widest.max(row.encode_utf16().count());
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
        let dpi =
            unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(view.hwnd) }.max(96) as usize;
        msg(
            view,
            PROCESS_LIST,
            0x0194,
            widest.saturating_mul(12 * dpi) / 96,
            0,
        );
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
        if let Ok(exe) = std::env::current_exe() {
            self.load_startup_transcription_config(view, &exe);
        }
        self.sync_full(view);
        self.initialize_recents(view);
        self.update_timer(view);
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

    fn sync_profile(&mut self, view: &View) {
        self.encoding_summary = encoding_label(&self.settings);
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
        let idle = self.session.is_none() && self.live.is_none();
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
        let running = self.session.is_some() || self.live.is_some();
        let recording = self.recording();
        let monitoring = self.settings.monitor_only && self.session.is_some();
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
        enabled(
            view,
            START,
            !self.closing && (recording || monitoring || self.retiring.len() < 4),
        );
        set_changed(
            view,
            START,
            if monitoring {
                "Stop monitoring"
            } else if recording {
                "Stop recording"
            } else {
                "Start recording"
            },
        );
        enabled(view, STOP, recording || monitoring);
        enabled(view, PAUSE, recording || monitoring);
        enabled(view, MARK, recording && !self.settings.monitor_only);
        enabled(view, TRANSCRIBE_FILES, !self.closing);
        enabled(view, BATCH_MANAGE, !self.closing);
        enabled(view, LIVE_TOGGLE, !self.closing && !monitoring);
        let started = self.live.as_ref().is_some_and(|w| w.started.is_some());
        let waiting = self.live.is_some() && !started;
        set_changed(
            view,
            LIVE_TOGGLE,
            if waiting {
                "Cancel transcription start"
            } else if started {
                "Stop transcription"
            } else {
                "Start transcription"
            },
        );
        enabled(view, STOP_BOTH, recording && started);

        let can_configure = self.live.is_none() && !self.closing;
        for id in [
            LIVE_ENDPOINT,
            LIVE_MODEL,
            LIVE_KEY_ENV,
            LIVE_OUTPUT_DIR,
            LIVE_BROWSE,
            LIVE_TIMESTAMPS,
            LOAD_LIVE_CONFIG,
            SAVE_LIVE_CONFIG,
        ] {
            enabled(view, id, can_configure);
        }
        self.recent_controls(view);
        enabled(
            view,
            LIVE_GATE,
            can_configure && audio_capture::live_wire::QUALIFIED_LIVE_GATE,
        );
        enabled(
            view,
            LIVE_SPEAKER_CONFIG,
            can_configure && audio_capture::live_wire::QUALIFIED_LIVE_SPEAKERS,
        );
        enabled(
            view,
            LIVE_SPEAKERS,
            !waiting && !self.closing && audio_capture::live_wire::QUALIFIED_LIVE_SPEAKERS,
        );
        enabled(view, LIVE_CANCEL_PENDING, !self.finishing.is_empty());
        view.set_visible(
            LIVE_PREVIEW,
            self.live.is_some() || !self.finishing.is_empty() || self.last_live.path.is_some(),
        );
        let has_path = self.displayed_path().is_some();
        enabled(view, LIVE_OPEN_TEXT, has_path);
        enabled(view, LIVE_OPEN_FOLDER, has_path);
        set_changed(
            view,
            PAUSE,
            if self.paused {
                "Resume recording"
            } else {
                "Pause recording"
            },
        );
        self.encoding_enabled(view);
    }

    fn show_status(&self, view: &View) {
        let status = if self.recording() || (self.settings.monitor_only && self.session.is_some()) {
            let elapsed = self
                .recording_started
                .map(|t| clock(t.elapsed().as_secs()))
                .unwrap_or_default();
            let state = self
                .session
                .as_ref()
                .map(|s| s.status())
                .unwrap_or_default();
            if self.settings.monitor_only {
                format!("{state}  {elapsed}  |  {}", self.settings.folder.display())
            } else {
                format!(
                    "{state}  {elapsed}  |  {}  |  {}",
                    self.encoding_summary,
                    self.settings.folder.display()
                )
            }
        } else if self.status.is_empty() {
            format!(
                "Ready  |  {}  |  {}",
                self.encoding_summary,
                self.settings.folder.display()
            )
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
        let status = if let Some(warning) = &self.recording_warning {
            format!("{status}  |  {warning}")
        } else {
            status
        };
        set_changed(view, STATUS, &status);
        let summary = self.scope_summary();
        set_changed(
            view,
            SOURCE_SUMMARY,
            if summary.is_empty() {
                "Select a program or microphone"
            } else {
                &summary
            },
        );
        self.show_live(view);
    }

    fn confirm_source_selection(&mut self, view: &View) -> bool {
        if self.missing.is_empty() {
            return true;
        }
        let prompt = format!(
            "These saved programs are unavailable or ambiguous: {}. Clear them and continue with the selected sources?",
            self.missing.join(", ")
        );
        let confirmed = unsafe {
            MessageBoxW(
                view.hwnd,
                wide(&prompt).as_ptr(),
                wide("Unresolved programs").as_ptr(),
                0x124,
            )
        } == 6;
        if confirmed {
            self.missing.clear();
        }
        confirmed
    }

    fn config(&self) -> Result<record::Config, String> {
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
            let same_name = self
                .processes
                .iter()
                .filter(|q| {
                    self.selected.contains_key(&q.pid) && q.name.eq_ignore_ascii_case(&p.name)
                })
                .count();
            let label = if same_name > 1 {
                format!("{} (PID {})", p.name, p.pid)
            } else {
                p.name.clone()
            };
            inputs.push((
                label,
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
        if config.inputs.is_empty() {
            return Err("Select a program, microphone, or system audio first.".into());
        }
        Ok(config)
    }

    fn recording(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.recording())
    }

    fn start_recording(&mut self) -> Result<(), String> {
        if self.session.is_none() && self.retiring.len() >= 4 {
            return Err("Wait for a previous recording to finish saving.".into());
        }
        if let Some(session) = &self.session {
            session.start_recording()?;
        } else {
            self.session = Some(record::Session::start(self.config()?)?);
        }
        self.recording_started = Some(Instant::now());
        self.encoding_summary = encoding_label(&self.settings);
        self.recording_warning = None;
        self.marker_count = 0;
        self.paused = false;
        self.remember_selection();
        Ok(())
    }

    fn scope_summary(&self) -> String {
        let mut names: Vec<_> = self
            .processes
            .iter()
            .filter(|p| self.selected.contains_key(&p.pid))
            .map(|p| format!("{} (PID {})", p.name, p.pid))
            .collect();
        if self.settings.system {
            names.push("System audio".into());
        }
        if let Some(id) = &self.settings.mic {
            let name = self
                .inputs
                .iter()
                .find(|(key, _)| key == id)
                .map(|(_, name)| name.as_str())
                .unwrap_or("Unavailable");
            names.push(format!("Microphone: {name}"));
        }
        names.join(" · ")
    }

    fn update_timer(&self, view: &View) {
        let needed = !self.close_ready() || self.recents.needs_tick();
        if self.timer_on.get() == needed {
            return;
        }
        unsafe {
            if needed {
                self.timer_on
                    .set(SetTimer(view.hwnd, SESSION_TIMER, 250, ptr::null()) != 0);
            } else {
                KillTimer(view.hwnd, SESSION_TIMER);
                self.timer_on.set(false);
            }
        }
    }

    pub fn command(&mut self, view: &View, id: i32, notification: u16) {
        let locked = self.session.is_some() || self.live.is_some();
        if self.closing {
            if notification == BN_CLICKED
                && matches!(id, LIVE_CANCEL_PENDING | LIVE_OPEN_TEXT | LIVE_OPEN_FOLDER)
            {
                self.live_command(view, id);
                self.show_status(view);
            }
            return;
        }
        if self.recent_command(view, id, notification) {
            self.update_timer(view);
            return;
        }
        if !locked
            && [SYSTEM, MIC, PROCESS_LIST, LOAD].contains(&id)
            && [LBN_SELCHANGE, BN_CLICKED].contains(&notification)
        {
            self.recents.manual_source_choice();
        }
        if !locked
            && ((id == MIC && notification == CBN_SELCHANGE)
                || (id == SYSTEM && notification == BN_CLICKED))
        {
            self.read_controls(view);
        }
        if !locked && id == PROCESS_LIST && notification == LBN_SELCHANGE {
            self.update_selection(view);
            self.show_status(view);
            return;
        }
        if !accepts_command(id, locked) {
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
            self.refresh_recent_sources(view);
            self.update_timer(view);
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
        } else if id == BATCH_MANAGE && notification == BN_CLICKED {
            if let Err(error) = launch_batch_manager() {
                self.status = error;
            }
        } else if id == LIVE_BROWSE && self.live.is_none() && notification == BN_CLICKED {
            if let Some(folder) = rfd::FileDialog::new()
                .set_directory(text(view, FOLDER))
                .pick_folder()
            {
                set_text(view, LIVE_OUTPUT_DIR, &folder.to_string_lossy());
            }
        } else if matches!(id, LOAD_LIVE_CONFIG | SAVE_LIVE_CONFIG)
            && self.live.is_none()
            && notification == BN_CLICKED
        {
            self.transcription_config_command(view, id);
        } else if [
            LIVE_TOGGLE,
            STOP_BOTH,
            LIVE_SPEAKERS,
            LIVE_CANCEL_PENDING,
            LIVE_OPEN_TEXT,
            LIVE_OPEN_FOLDER,
        ]
        .contains(&id)
            && notification == BN_CLICKED
        {
            self.live_command(view, id);
            self.state_controls(view);
            self.update_timer(view);
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
            if self.recording() || (self.settings.monitor_only && self.session.is_some()) {
                self.stop_recording(view);
                return;
            }
            self.read_controls(view);
            if !self.confirm_source_selection(view) {
                return;
            }
            if let Err(error) = self.start_recording() {
                self.status = error;
            }
            self.update_timer(view);
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
            self.stop_recording(view);
        }
        self.show_status(view);
    }

    fn retire_if_stopped(&mut self) {
        if self
            .session
            .as_ref()
            .is_some_and(|s| !s.recording() && !s.transcribing())
        {
            self.retiring
                .push(self.session.take().expect("stopped capture"));
        }
    }

    fn stop_recording(&mut self, view: &View) {
        if let Some(session) = &self.session {
            let stop = if self.settings.monitor_only {
                session.stop_both()
            } else {
                session.stop_recording()
            };
            if let Err(error) = stop {
                self.status = error;
                self.show_status(view);
                return;
            }
        }
        self.status = if self.settings.monitor_only {
            "Monitoring stopped".into()
        } else {
            format!("Finishing recording  |  {}", self.settings.folder.display())
        };
        self.retire_if_stopped();
        self.recording_started = None;
        self.paused = false;
        self.state_controls(view);
        self.show_status(view);
    }

    fn remember_close_error(&mut self, detail: &str) {
        if self.closing && !self.close_errors.iter().any(|error| error == detail) {
            self.close_errors.push(detail.to_owned());
        }
    }

    fn recording_completed(&mut self, result: Result<PathBuf, String>) {
        let status = match result {
            Ok(path) => format!("Recording saved  |  {}", path.display()),
            Err(error) => {
                let detail = format!("Recording failed  |  {error}");
                self.remember_close_error(&detail);
                if self.recording() {
                    self.recording_warning = Some(format!("Earlier recording failed: {error}"));
                }
                detail
            }
        };
        if !self.recording() {
            self.status = status;
        }
    }

    fn session_ended(&mut self, mut session: record::Session) {
        let joined = session.stop();
        while let Some(result) = session.take_recording_result() {
            self.recording_completed(result);
        }
        if let Err(error) = joined {
            self.recording_completed(Err(error));
        }
    }

    fn take_close_error(&mut self) -> Option<String> {
        if self.close_errors.is_empty() {
            return None;
        }
        if let Some(recovery) = self.close_recovery.take() {
            self.last_live = recovery;
            self.live_note = "Review transcript recovery".into();
        }
        self.closing = false;
        let errors = std::mem::take(&mut self.close_errors).join("\n");
        Some(format!(
            "{errors}\n\nReview the output. Close the window again to exit."
        ))
    }

    pub fn tick(&mut self, view: &View) {
        if self.closing && (self.session.is_some() || self.live.is_some()) {
            self.stop_both();
        }
        let sources_changed = self.poll_recent_sources(view);
        if self.close_ready() && !self.closing && !sources_changed {
            self.update_timer(view);
            return;
        }
        let mut index = 0;
        while index < self.retiring.len() {
            if self.retiring[index].is_finished() {
                let ended = self.retiring.remove(index);
                self.session_ended(ended);
            } else {
                index += 1;
            }
        }
        if self.session.as_ref().is_some_and(|s| s.is_finished()) {
            if let Some(ended) = self.session.take() {
                self.session_ended(ended);
            }
            self.paused = false;
            self.recording_started = None;
            set_levels(view, "Source levels appear during capture.");
        }
        if self.recording_started.is_some() && !self.settings.monitor_only && !self.recording() {
            self.recording_started = None;
            if let Some(session) = &self.session {
                self.status = session.status();
            }
        }
        self.poll_live(view);
        if let Some(session) = &self.session {
            set_levels(view, &session.levels());
            if let Some(result) = session.take_recording_result() {
                self.recording_completed(result);
            }
        }
        self.state_controls(view);
        self.show_status(view);
        self.update_timer(view);
        if self.closing && self.close_ready() {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    view.hwnd,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                    0,
                    0,
                );
            }
        }
    }

    pub fn close_ready(&self) -> bool {
        self.session.is_none()
            && self.live.is_none()
            && self.finishing.is_empty()
            && self.retiring.is_empty()
    }

    pub fn shutdown(&mut self, view: &View) -> Result<(), String> {
        let mut errors = Vec::new();
        if !self.close_ready() {
            if !self.closing {
                self.closing = true;
                self.stop_both();
                self.state_controls(view);
                self.show_status(view);
                self.update_timer(view);
            }
            return Ok(());
        }
        self.paused = false;
        if let Some(error) = self.take_close_error() {
            self.status = error.clone();
            self.state_controls(view);
            self.show_status(view);
            return Err(error);
        }
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
            self.closing = false;
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
    fn close_reports_failed_publication_once_and_restores_the_recovery_path() {
        let mut app = Controller::from_settings(prefs::Settings::default(), String::new(), false);
        app.closing = true;
        app.recording_completed(Err("first.wav: disk full".into()));
        let partial = PathBuf::from("first.partial.txt");
        app.transcript_completed(
            LiveStatus {
                path: Some(partial.clone()),
                ..Default::default()
            },
            Some("Output folder is unavailable".into()),
        );
        app.recording_completed(Ok(PathBuf::from("second.wav")));
        app.transcript_completed(
            LiveStatus {
                state: "complete".into(),
                path: Some("second.txt".into()),
                ..Default::default()
            },
            None,
        );
        let error = app
            .take_close_error()
            .expect("A failed save must interrupt close");
        assert!(error.contains("first.wav: disk full"));
        assert!(error.contains("Output folder is unavailable"));
        assert!(error.contains("first.partial.txt"));
        assert_eq!(app.displayed_path(), Some(partial));
        assert!(!app.closing);
        assert!(app.close_ready());
        assert!(
            app.take_close_error().is_none(),
            "An explicit second close can exit"
        );
    }

    #[test]
    fn completed_file_and_live_controls_are_available_during_recording() {
        assert!(accepts_command(TRANSCRIBE_FILES, false));
        assert!(accepts_command(TRANSCRIBE_FILES, true));
        assert!(accepts_command(LIVE_TOGGLE, true));
        assert!(accepts_command(STOP_BOTH, true));
        assert!(accepts_command(LOAD_LIVE_CONFIG, true));
        assert!(accepts_command(SAVE_LIVE_CONFIG, true));
        assert!(!accepts_command(PROCESS_LIST, true));
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
    fn recording_readout_preserves_profile_and_shows_effective_wav_rate() {
        let mut settings = prefs::Settings::default();
        let original = settings.clone();
        assert_eq!(encoding_label(&settings), "Speech (Opus 32 kbps mono)");
        assert_eq!(settings, original);
        settings.format = "opus-speech".into();
        assert_eq!(encoding_label(&settings), "Speech DTX (Opus 32 kbps mono)");
        settings.format = "wav".into();
        settings.wav_rate = 16_000;
        assert_eq!(encoding_label(&settings), "WAV 16000 Hz mono");
        settings.wav_rate = 0;
        assert_eq!(encoding_label(&settings), "WAV 48000 Hz mono (Auto)");
        assert_eq!(settings.wav_rate, 0);
        settings.channels = 2;
        settings.wav_rate = 44_100;
        assert_eq!(encoding_label(&settings), "WAV 44100 Hz stereo");
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
