//! Native Windows controls and message dispatch for the recorder.
use crate::{
    native_controller::{Controller, TRANSCRIPTION_SETUP_NEEDED},
    native_layout,
};
use audio_capture::live_wire::{QUALIFIED_LIVE_GATE, QUALIFIED_LIVE_SPEAKERS};
use audio_capture::transcription_config::{MAX_ENDPOINT_UNITS, MAX_FIELD_UNITS, MAX_PATH_UNITS};
use std::{
    cell::Cell,
    collections::HashMap,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        COLOR_WINDOW, CreateFontW, DeleteObject, HFONT, RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE,
        RedrawWindow, ScreenToClient, UpdateWindow,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::{EM_LIMITTEXT, EM_SETCUEBANNER, SetScrollInfo},
        HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
            SetProcessDpiAwarenessContext,
        },
        Input::KeyboardAndMouse::{GetFocus, IsWindowEnabled, SetFocus},
        Shell::{
            NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
        },
        WindowsAndMessaging::*,
    },
};

pub const PRESET_LABEL: i32 = 101;
pub const PRESET: i32 = 102;
pub const SAVE: i32 = 103;
pub const LOAD: i32 = 104;
pub const DELETE: i32 = 105;
pub const PROCESS_LABEL: i32 = 106;
pub const PROCESS_LIST: i32 = 107;
pub const REFRESH: i32 = 108;
pub const FILTER: i32 = 109;
pub const SYSTEM: i32 = 110;
pub const FORMAT_LABEL: i32 = 111;
pub const FORMAT: i32 = 112;
pub const BITRATE_LABEL: i32 = 113;
pub const BITRATE: i32 = 114;
pub const SAMPLE_LABEL: i32 = 115;
pub const SAMPLE: i32 = 116;
pub const CHANNEL_LABEL: i32 = 117;
pub const CHANNEL: i32 = 118;
pub const MIC_LABEL: i32 = 119;
pub const MIC: i32 = 120;
pub const MONITOR_LABEL: i32 = 121;
pub const MONITOR: i32 = 122;
pub const MONITOR_ONLY: i32 = 123;
pub const APP_GAIN_LABEL: i32 = 124;
pub const APP_GAIN: i32 = 125;
pub const MIC_GAIN_LABEL: i32 = 126;
pub const MIC_GAIN: i32 = 127;
pub const MODE_LABEL: i32 = 128;
pub const MODE: i32 = 129;
pub const SILENCE: i32 = 130;
pub const FOLDER_LABEL: i32 = 131;
pub const FOLDER: i32 = 132;
pub const BROWSE: i32 = 133;
pub const START: i32 = 134;
pub const PAUSE: i32 = 135;
pub const STOP: i32 = 136;
pub const STATUS: i32 = 137;
pub const LEVELS_LABEL: i32 = 138;
pub const LEVELS: i32 = 139;
pub const MARK: i32 = 140;
pub const TRANSCRIBE_FILES: i32 = 141;
pub const PROFILE_LABEL: i32 = 142;
pub const PROFILE: i32 = 143;
pub const PROFILE_HELP: i32 = 144;
pub const LIVE_TOGGLE: i32 = 145;
pub const STOP_BOTH: i32 = 146;
pub const LIVE_STATUS: i32 = 147;
pub const LIVE_PREVIEW: i32 = 148;
pub const LIVE_ENDPOINT: i32 = 149;
pub const LIVE_MODEL: i32 = 150;
pub const LIVE_KEY_ENV: i32 = 151;
pub const LIVE_GATE: i32 = 152;
pub const LIVE_SPEAKERS: i32 = 153;
pub const LIVE_SPEAKER_CONFIG: i32 = 154;
pub const LIVE_OPEN_TEXT: i32 = 155;
pub const LIVE_OPEN_FOLDER: i32 = 156;
pub const LIVE_CANCEL_PENDING: i32 = 157;
pub const LIVE_OUTPUT_DIR: i32 = 158;
pub const LIVE_BROWSE: i32 = 159;
pub const BATCH_MANAGE: i32 = 160;
pub const RECENT_PICKER: i32 = 161;
pub const USE_RECENT: i32 = 162;
pub const FORGET_RECENT: i32 = 163;
pub const CLEAR_RECENT: i32 = 164;
pub const AUTO_SELECT_RECENT: i32 = 165;
pub const SOURCE_SUMMARY: i32 = 166;
pub const SOURCES_SECTION: i32 = 167;
pub const RECORDING_SECTION: i32 = 168;
pub const LIVE_SECTION: i32 = 169;
pub const BATCH_SECTION: i32 = 170;
pub const SELECT_PROGRAMS: i32 = 171;
pub const RECENT_OPTIONS: i32 = 172;
pub const RECORDING_SETTINGS: i32 = 173;
pub const TRANSCRIPTION_SETTINGS: i32 = 174;
pub const LIVE_ENDPOINT_LABEL: i32 = 175;
pub const LIVE_MODEL_LABEL: i32 = 176;
pub const LIVE_KEY_ENV_LABEL: i32 = 177;
pub const LIVE_SPEAKER_CONFIG_LABEL: i32 = 178;
pub const LIVE_OUTPUT_DIR_LABEL: i32 = 179;
pub const LIVE_PREVIEW_LABEL: i32 = 180;
pub const RECENT_LABEL: i32 = 181;
pub const RECORDING_SETTINGS_SECTION: i32 = 182;
pub const TRANSCRIPTION_SETTINGS_SECTION: i32 = 183;
pub const RECORDING_STATUS_LABEL: i32 = 184;
pub const LIVE_STATUS_LABEL: i32 = 185;
pub const LIVE_TIMESTAMPS: i32 = 186;
pub const LOAD_LIVE_CONFIG: i32 = 187;
pub const SAVE_LIVE_CONFIG: i32 = 188;
pub const LOCAL_INPUT_LABEL: i32 = 190;
pub const LOCAL_INPUT: i32 = 191;
pub const LOCAL_INPUT_BROWSE: i32 = 192;
pub const LOCAL_OUTPUT_LABEL: i32 = 193;
pub const LOCAL_OUTPUT: i32 = 194;
pub const LOCAL_OUTPUT_BROWSE: i32 = 195;
pub const LOCAL_PREVIEW: i32 = 196;
pub const LOCAL_APPROVE: i32 = 197;
pub const LOCAL_CANCEL: i32 = 198;
pub const LOCAL_STATUS: i32 = 199;
pub const LOCAL_REVIEW: i32 = 200;
pub const LOCAL_IMPORT: i32 = 201;
pub const LOCAL_MODEL_LABEL: i32 = 202;
pub const LOCAL_MODEL: i32 = 203;
pub const LOCAL_MODEL_BROWSE: i32 = 204;
pub const LOCAL_NEXT: i32 = 205;
pub const LOCAL_STATUS_LABEL: i32 = 206;
pub const LOCAL_REVIEW_LABEL: i32 = 207;
pub const ENDPOINT_FILES: i32 = 208;
pub const ENDPOINT_QUEUE: i32 = 209;
pub const ENDPOINT_REPLY_LABEL: i32 = 210;
pub const ENDPOINT_REPLY: i32 = 211;
pub const ENDPOINT_SEND: i32 = 212;
pub const ENDPOINT_CLOSE: i32 = 213;
pub const LOCAL_PREVIOUS: i32 = 214;

pub const LOCAL_PANEL: &[i32] = &[
    LOCAL_INPUT_LABEL,
    LOCAL_INPUT,
    LOCAL_INPUT_BROWSE,
    LOCAL_OUTPUT_LABEL,
    LOCAL_OUTPUT,
    LOCAL_OUTPUT_BROWSE,
    LOCAL_PREVIEW,
    LOCAL_APPROVE,
    LOCAL_CANCEL,
    LOCAL_STATUS_LABEL,
    LOCAL_STATUS,
    LOCAL_REVIEW_LABEL,
    LOCAL_REVIEW,
    LOCAL_PREVIOUS,
    LOCAL_NEXT,
    LOCAL_MODEL_LABEL,
    LOCAL_MODEL,
    LOCAL_MODEL_BROWSE,
    LOCAL_IMPORT,
    ENDPOINT_FILES,
    ENDPOINT_QUEUE,
    ENDPOINT_REPLY_LABEL,
    ENDPOINT_REPLY,
    ENDPOINT_SEND,
    ENDPOINT_CLOSE,
];
pub const PROFILE_WIDTH: i32 = 310;

const TRAY_MESSAGE: u32 = WM_APP + 14;
const STATIC_LITERAL: u32 = 0x0080;
const STATIC_ELLIPSIS: u32 = 0x4000;
const CLICKED: u16 = 0;
const HEADINGS: &[i32] = &[
    SOURCES_SECTION,
    RECORDING_SECTION,
    LIVE_SECTION,
    BATCH_SECTION,
    RECORDING_SETTINGS_SECTION,
    TRANSCRIPTION_SETTINGS_SECTION,
];

const PROGRAM_CONTROLS: &[i32] = &[PROCESS_LABEL, PROCESS_LIST, REFRESH, FILTER, SYSTEM];
// Keep focus on the disclosure buttons when the user collapses a section.
const RECENT_CONTROLS: &[i32] = &[FORGET_RECENT, CLEAR_RECENT, AUTO_SELECT_RECENT];
const RECORDING_CONTROLS: &[i32] = &[
    RECORDING_SETTINGS_SECTION,
    PROFILE_LABEL,
    PROFILE,
    PROFILE_HELP,
    FORMAT_LABEL,
    FORMAT,
    BITRATE_LABEL,
    BITRATE,
    SAMPLE_LABEL,
    SAMPLE,
    CHANNEL_LABEL,
    CHANNEL,
    PRESET_LABEL,
    PRESET,
    SAVE,
    LOAD,
    DELETE,
    MONITOR_LABEL,
    MONITOR,
    MONITOR_ONLY,
    APP_GAIN_LABEL,
    APP_GAIN,
    MIC_GAIN_LABEL,
    MIC_GAIN,
    MODE_LABEL,
    MODE,
    SILENCE,
    FOLDER_LABEL,
    FOLDER,
    BROWSE,
];
const TRANSCRIPTION_CONTROLS: &[i32] = &[
    TRANSCRIPTION_SETTINGS_SECTION,
    LOAD_LIVE_CONFIG,
    SAVE_LIVE_CONFIG,
    LIVE_ENDPOINT_LABEL,
    LIVE_ENDPOINT,
    LIVE_MODEL_LABEL,
    LIVE_MODEL,
    LIVE_KEY_ENV_LABEL,
    LIVE_KEY_ENV,
    LIVE_OUTPUT_DIR_LABEL,
    LIVE_OUTPUT_DIR,
    LIVE_BROWSE,
    LIVE_GATE,
    LIVE_TIMESTAMPS,
    LIVE_SPEAKERS,
    LIVE_SPEAKER_CONFIG_LABEL,
    LIVE_SPEAKER_CONFIG,
];

#[link(name = "user32")]
unsafe extern "system" {
    fn NotifyWinEvent(event: u32, hwnd: HWND, object: i32, child: i32);
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn ui_font(dpi: u32, points: i32, weight: i32) -> HFONT {
    let face = wide("Segoe UI");
    unsafe {
        CreateFontW(
            -((points * dpi as i32 + 36) / 72),
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            1,
            0,
            0,
            5,
            0,
            face.as_ptr(),
        )
    }
}

fn focus_scroll(scroll: i32, top: i32, bottom: i32, height: i32) -> i32 {
    if top < 0 {
        scroll + top - 8
    } else if bottom > height {
        scroll + bottom - height + 8
    } else {
        scroll
    }
}

pub struct View {
    pub hwnd: HWND,
    controls: HashMap<i32, HWND>,
    layout_dirty: Cell<bool>,
    show_live_settings: Cell<bool>,
}

impl View {
    pub fn get(&self, id: i32) -> HWND {
        self.controls.get(&id).copied().unwrap_or(null_mut())
    }
    pub fn place(&self, id: i32, x: i32, y: i32, w: i32, h: i32) {
        let combo = [
            PROFILE,
            PRESET,
            FORMAT,
            BITRATE,
            SAMPLE,
            CHANNEL,
            MIC,
            MONITOR,
            APP_GAIN,
            MIC_GAIN,
            MODE,
            RECENT_PICKER,
        ]
        .contains(&id);
        let height = if combo { h.max(1) * 9 } else { h.max(1) };
        unsafe {
            SetWindowPos(
                self.get(id),
                null_mut(),
                x,
                y,
                w.max(1),
                height,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
            );
        }
    }
    pub fn visible(&self, id: i32) -> bool {
        unsafe { (GetWindowLongPtrW(self.get(id), GWL_STYLE) as u32 & WS_VISIBLE) != 0 }
    }

    pub fn set_visible(&self, id: i32, visible: bool) {
        if self.visible(id) != visible {
            unsafe { ShowWindow(self.get(id), if visible { SW_SHOWNA } else { SW_HIDE }) };
            self.layout_dirty.set(true);
        }
    }

    pub fn show_transcription_settings(&self) {
        self.show_live_settings.set(true);
    }

    fn set_panel(&self, ids: &[i32], visible: bool) {
        for id in ids {
            self.set_visible(*id, visible);
        }
    }

    fn caption(&self, id: i32, text: &str) {
        unsafe { SetWindowTextW(self.get(id), wide(text).as_ptr()) };
    }

    fn read_caption(&self, id: i32) -> [u16; 32] {
        let mut caption = [0; 32];
        unsafe { GetWindowTextW(self.get(id), caption.as_mut_ptr(), caption.len() as i32) };
        caption
    }

    fn announce_status(&self, id: i32) {
        unsafe {
            NotifyWinEvent(
                EVENT_OBJECT_VALUECHANGE,
                self.get(id),
                OBJID_CLIENT,
                CHILDID_SELF as i32,
            )
        };
    }
}

struct Window {
    view: View,
    controller: Controller,
    scroll: i32,
    content: i32,
    font: HFONT,
    heading_font: HFONT,
    tray: bool,
    programs_open: bool,
    recent_open: bool,
    recording_open: bool,
    transcription_open: bool,
    recording_controls: Option<([u16; 32], [u16; 32])>,
    transcription_controls: Option<([u16; 32], bool, bool)>,
}

impl Window {
    unsafe fn update_font(&mut self) {
        let dpi = unsafe { GetDpiForWindow(self.view.hwnd) }.max(96);
        let font = ui_font(dpi, 9, 400);
        let heading_font = ui_font(dpi, 10, 600);
        if !font.is_null() {
            for (id, hwnd) in &self.view.controls {
                let control_font = if HEADINGS.contains(id) && !heading_font.is_null() {
                    heading_font
                } else {
                    font
                };
                unsafe {
                    SendMessageW(*hwnd, WM_SETFONT, control_font as usize, 1);
                }
            }
            for old in [self.font, self.heading_font] {
                if !old.is_null() {
                    unsafe { DeleteObject(old as _) };
                }
            }
            self.font = font;
            self.heading_font = heading_font;
        } else if !heading_font.is_null() {
            unsafe { DeleteObject(heading_font as _) };
        }
        let profile_width = (PROFILE_WIDTH as usize * dpi as usize + 48) / 96;
        unsafe {
            SendMessageW(self.view.get(PROFILE), CB_SETDROPPEDWIDTH, profile_width, 0);
            SendMessageW(
                self.view.get(RECENT_PICKER),
                CB_SETDROPPEDWIDTH,
                profile_width,
                0,
            );
        }
    }

    fn update_activity_visibility(&self) {
        for id in [
            PAUSE,
            MARK,
            STOP_BOTH,
            LIVE_CANCEL_PENDING,
            LIVE_OPEN_TEXT,
            LIVE_OPEN_FOLDER,
        ] {
            let enabled = unsafe { IsWindowEnabled(self.view.get(id)) != 0 };
            self.view.set_visible(id, enabled);
        }
        self.view
            .set_visible(LIVE_PREVIEW_LABEL, self.view.visible(LIVE_PREVIEW));
    }

    fn announce_activity_changes(&mut self) {
        let recording = (self.view.read_caption(START), self.view.read_caption(PAUSE));
        let transcription = (
            self.view.read_caption(LIVE_TOGGLE),
            self.view.visible(LIVE_CANCEL_PENDING),
            self.view.visible(LIVE_OPEN_TEXT),
        );
        if self
            .recording_controls
            .replace(recording)
            .is_some_and(|old| old != recording)
        {
            self.view.announce_status(STATUS);
        }
        if self
            .transcription_controls
            .replace(transcription)
            .is_some_and(|old| old != transcription)
        {
            self.view.announce_status(LIVE_STATUS);
        }
    }

    fn set_transcription_panel(&mut self, open: bool) {
        self.transcription_open = open;
        self.view.set_panel(TRANSCRIPTION_CONTROLS, open);
        self.view.caption(
            TRANSCRIPTION_SETTINGS,
            if open {
                "Hide transcription settings"
            } else {
                "Transcription settings"
            },
        );
    }

    unsafe fn command(&mut self, id: i32, notification: u16) {
        let focus = match (id, notification) {
            (TRANSCRIBE_FILES | BATCH_MANAGE, CLICKED) => {
                let open = id == BATCH_MANAGE || !self.view.visible(LOCAL_INPUT);
                self.view.set_panel(LOCAL_PANEL, open);
                self.controller
                    .local_panel(&self.view, open, id == BATCH_MANAGE);
                self.view.caption(
                    TRANSCRIBE_FILES,
                    if open {
                        "Hide file transcription"
                    } else {
                        "Transcribe files…"
                    },
                );
                Some(if open && id == BATCH_MANAGE {
                    LOCAL_STATUS
                } else if open {
                    LOCAL_INPUT
                } else {
                    TRANSCRIBE_FILES
                })
            }
            (SELECT_PROGRAMS, CLICKED) => {
                self.programs_open = !self.programs_open;
                self.view.set_panel(PROGRAM_CONTROLS, self.programs_open);
                self.view.caption(
                    id,
                    if self.programs_open {
                        "Hide programs"
                    } else {
                        "Select programs…"
                    },
                );
                self.programs_open.then_some(PROCESS_LIST)
            }
            (RECENT_OPTIONS, CLICKED) => {
                self.recent_open = !self.recent_open;
                self.view.set_panel(RECENT_CONTROLS, self.recent_open);
                self.view.caption(
                    id,
                    if self.recent_open {
                        "Hide recent settings"
                    } else {
                        "Manage recent"
                    },
                );
                None
            }
            (RECORDING_SETTINGS, CLICKED) => {
                self.recording_open = !self.recording_open;
                self.view.set_panel(RECORDING_CONTROLS, self.recording_open);
                self.view.caption(
                    id,
                    if self.recording_open {
                        "Hide recording settings"
                    } else {
                        "Recording settings"
                    },
                );
                Some(if self.recording_open { PROFILE } else { id })
            }
            (TRANSCRIPTION_SETTINGS, CLICKED) => {
                self.set_transcription_panel(!self.transcription_open);
                Some(if self.transcription_open {
                    LOAD_LIVE_CONFIG
                } else {
                    id
                })
            }
            _ => {
                self.controller.command(&self.view, id, notification);
                None
            }
        };
        unsafe { self.refresh_view(focus) };
    }

    unsafe fn refresh_view(&mut self, mut focus: Option<i32>) {
        if self.view.show_live_settings.replace(false) {
            self.set_transcription_panel(true);
            focus = Some(LIVE_ENDPOINT);
        }
        self.update_activity_visibility();
        self.announce_activity_changes();
        if self.view.layout_dirty.get() {
            unsafe { self.layout() };
        }
        if let Some(id) = focus {
            let control = self.view.get(id);
            if unsafe { IsWindowEnabled(control) != 0 && IsWindowVisible(control) != 0 } {
                unsafe {
                    SetFocus(control);
                }
            }
            // During capture the settings are visible but their fields cannot take focus.
            let (first, last) = match id {
                PROFILE => (RECORDING_SETTINGS_SECTION, PROFILE),
                LOAD_LIVE_CONFIG | LIVE_ENDPOINT => (TRANSCRIPTION_SETTINGS_SECTION, LIVE_ENDPOINT),
                _ => (id, id),
            };
            unsafe { self.reveal_controls(self.view.get(first), self.view.get(last)) };
        }
    }

    unsafe fn layout(&mut self) {
        let mut rect: RECT = unsafe { zeroed() };
        unsafe {
            GetClientRect(self.view.hwnd, &mut rect);
        }
        let width = rect.right.max(1);
        let height = rect.bottom.max(1);
        let dpi = unsafe { GetDpiForWindow(self.view.hwnd) }.max(96);
        let content = native_layout::layout(&self.view, width, height, dpi, self.scroll);
        let clamped = self.scroll.clamp(0, (content - height).max(0));
        if clamped != self.scroll {
            self.scroll = clamped;
            native_layout::layout(&self.view, width, height, dpi, self.scroll);
        }
        self.content = content;
        self.view.layout_dirty.set(false);
        let info = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: content.saturating_sub(1),
            nPage: height as u32,
            nPos: self.scroll,
            nTrackPos: 0,
        };
        unsafe {
            SetScrollInfo(self.view.hwnd, SB_VERT, &info, 1);
            // A control can move into another control's former rectangle after
            // preview, status, or settings visibility changes. Clear those pixels.
            RedrawWindow(
                self.view.hwnd,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            );
        }
    }
    unsafe fn scroll_to(&mut self, value: i32) {
        let mut client: RECT = unsafe { zeroed() };
        unsafe { GetClientRect(self.view.hwnd, &mut client) };
        let clamped = value.clamp(0, (self.content - client.bottom).max(0));
        if self.scroll != clamped {
            self.scroll = clamped;
            unsafe { self.layout() };
        }
    }
    unsafe fn ensure_focus_visible(&mut self) {
        let mut focus = unsafe { GetFocus() };
        while !focus.is_null() && !self.view.controls.values().any(|h| *h == focus) {
            focus = unsafe { GetParent(focus) };
            if focus == self.view.hwnd {
                return;
            }
        }
        if focus.is_null() {
            return;
        }
        unsafe { self.reveal_controls(focus, focus) };
    }

    unsafe fn reveal_controls(&mut self, first: HWND, last: HWND) {
        let mut first_bounds: RECT = unsafe { zeroed() };
        let mut last_bounds: RECT = unsafe { zeroed() };
        let mut client: RECT = unsafe { zeroed() };
        if unsafe {
            GetWindowRect(first, &mut first_bounds) == 0
                || GetWindowRect(last, &mut last_bounds) == 0
                || GetClientRect(self.view.hwnd, &mut client) == 0
        } {
            return;
        }
        let mut origin = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        if unsafe { ScreenToClient(self.view.hwnd, &mut origin) } == 0 {
            return;
        }
        let top = first_bounds.top + origin.y;
        let bottom = last_bounds.bottom + origin.y;
        let mut scroll = focus_scroll(self.scroll, top, bottom, client.bottom);
        if first != last && (top < 0 || bottom > client.bottom) {
            // Give the opened panel room for activity rows that may appear above it.
            scroll = self.scroll + top - 8;
        }
        if scroll != self.scroll {
            unsafe { self.scroll_to(scroll) };
        }
    }
}

unsafe fn tray(hwnd: HWND, action: u32) -> bool {
    let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = TRAY_MESSAGE;
    data.hIcon = unsafe { LoadIconW(null_mut(), IDI_APPLICATION) };
    let text = wide("Audio Capture");
    data.szTip[..text.len()].copy_from_slice(&text);
    unsafe { Shell_NotifyIconW(action, &data) != 0 }
}

#[derive(Clone, Copy, Default)]
struct DispatchGate {
    active: bool,
    close: bool,
    resize: bool,
    minimized: bool,
    dpi: Option<[i32; 4]>,
}

thread_local! {
    static GATE: std::cell::Cell<DispatchGate> = std::cell::Cell::new(DispatchGate::default());
}

const APPLY_DEFERRED: u32 = WM_APP + 15;

unsafe fn guarded(hwnd: HWND, f: impl FnOnce()) {
    GATE.with(|slot| {
        let mut gate = slot.get();
        gate.active = true;
        slot.set(gate);
    });
    f();
    let pending = GATE.with(|slot| {
        let mut gate = slot.get();
        gate.active = false;
        let pending = gate;
        gate.close = false;
        slot.set(gate);
        pending
    });
    if pending.close {
        unsafe {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    } else if pending.resize || pending.dpi.is_some() {
        unsafe {
            PostMessageW(hwnd, APPLY_DEFERRED, 0, 0);
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_GETMINMAXINFO && lp != 0 {
        let info = unsafe { &mut *(lp as *mut MINMAXINFO) };
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as i32;
        info.ptMinTrackSize.x = info.ptMinTrackSize.x.max(280 * dpi / 96);
        info.ptMinTrackSize.y = info.ptMinTrackSize.y.max(260 * dpi / 96);
        return 0;
    }
    // Synchronous control messages and modal dialogs can re-enter this procedure.
    // The gate lives separately from Window, so nested callbacks never borrow it.
    let reentrant = GATE.with(|slot| {
        let mut gate = slot.get();
        if !gate.active {
            return false;
        }
        match msg {
            WM_CLOSE => gate.close = true,
            WM_SIZE => {
                gate.resize = true;
                gate.minimized = wp as u32 == SIZE_MINIMIZED;
            }
            WM_DPICHANGED if lp != 0 => {
                let rect = unsafe { &*(lp as *const RECT) };
                gate.dpi = Some([rect.left, rect.top, rect.right, rect.bottom]);
                gate.resize = true;
            }
            _ => {}
        }
        slot.set(gate);
        true
    });
    if reentrant {
        return match msg {
            WM_COMMAND | WM_CLOSE | WM_SIZE | WM_DPICHANGED | WM_VSCROLL | WM_MOUSEWHEEL
            | WM_TIMER | TRAY_MESSAGE | APPLY_DEFERRED => 0,
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        };
    }

    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Window };
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    match msg {
        WM_COMMAND => {
            let id = (wp & 0xffff) as i32;
            let notification = ((wp >> 16) & 0xffff) as u16;
            unsafe {
                guarded(hwnd, || {
                    (*ptr).command(id, notification);
                });
            }
            0
        }
        WM_SIZE => {
            unsafe {
                guarded(hwnd, || {
                    if wp as u32 == SIZE_MINIMIZED && (*ptr).tray {
                        ShowWindow(hwnd, SW_HIDE);
                    } else {
                        (*ptr).layout();
                    }
                });
            }
            0
        }
        WM_DPICHANGED => {
            let rect = unsafe { *(lp as *const RECT) };
            unsafe {
                guarded(hwnd, || {
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    (*ptr).update_font();
                    (*ptr).layout();
                });
            }
            0
        }
        APPLY_DEFERRED => {
            let pending = GATE.with(|slot| {
                let mut gate = slot.get();
                let pending = gate;
                gate.resize = false;
                gate.minimized = false;
                gate.dpi = None;
                slot.set(gate);
                pending
            });
            unsafe {
                guarded(hwnd, || {
                    if let Some([left, top, right, bottom]) = pending.dpi {
                        SetWindowPos(
                            hwnd,
                            null_mut(),
                            left,
                            top,
                            right - left,
                            bottom - top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                        (*ptr).update_font();
                    }
                    if pending.resize && pending.minimized && (*ptr).tray {
                        ShowWindow(hwnd, SW_HIDE);
                    } else if pending.resize {
                        (*ptr).layout();
                    }
                });
            }
            0
        }
        WM_VSCROLL => {
            unsafe {
                guarded(hwnd, || {
                    let mut info: SCROLLINFO = zeroed();
                    info.cbSize = size_of::<SCROLLINFO>() as u32;
                    info.fMask = SIF_TRACKPOS | SIF_PAGE;
                    GetScrollInfo(hwnd, SB_VERT, &mut info);
                    let old = (*ptr).scroll;
                    let next = match (wp & 0xffff) as i32 {
                        SB_LINEUP => old - 32,
                        SB_LINEDOWN => old + 32,
                        SB_PAGEUP => old - info.nPage as i32,
                        SB_PAGEDOWN => old + info.nPage as i32,
                        SB_THUMBPOSITION | SB_THUMBTRACK => info.nTrackPos,
                        SB_TOP => 0,
                        SB_BOTTOM => i32::MAX,
                        _ => old,
                    };
                    (*ptr).scroll_to(next);
                });
            }
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp >> 16) as u16 as i16) as i32;
            unsafe {
                guarded(hwnd, || {
                    (*ptr).scroll_to((*ptr).scroll.saturating_sub(delta))
                });
            }
            0
        }
        WM_TIMER => {
            unsafe {
                guarded(hwnd, || {
                    (*ptr).controller.tick(&(*ptr).view);
                    (*ptr).refresh_view(None);
                });
            }
            0
        }
        TRAY_MESSAGE => {
            if lp as u32 == WM_LBUTTONUP || lp as u32 == WM_LBUTTONDBLCLK {
                unsafe {
                    guarded(hwnd, || {
                        ShowWindow(hwnd, SW_RESTORE);
                        SetForegroundWindow(hwnd);
                    });
                }
            }
            0
        }
        WM_CLOSE => {
            unsafe {
                let mut close_result = Ok(());
                let mut close_ready = false;
                guarded(hwnd, || {
                    close_result = (*ptr).controller.shutdown(&(*ptr).view);
                    close_ready = (*ptr).controller.close_ready();
                    if !close_ready || close_result.is_err() {
                        (*ptr).refresh_view(None);
                    }
                });
                match close_result {
                    Ok(()) if close_ready => {
                        DestroyWindow(hwnd);
                    }
                    Ok(()) => {}
                    Err(error) => {
                        MessageBoxW(
                            hwnd,
                            wide(&error).as_ptr(),
                            wide("Review output before closing").as_ptr(),
                            MB_OK | MB_ICONERROR,
                        );
                    }
                }
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                GATE.with(|slot| slot.set(DispatchGate::default()));
                KillTimer(hwnd, 1);
                if (*ptr).tray {
                    tray(hwnd, NIM_DELETE);
                }
                if !(*ptr).font.is_null() {
                    DeleteObject((*ptr).font as _);
                }
                if !(*ptr).heading_font.is_null() {
                    DeleteObject((*ptr).heading_font as _);
                }
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ptr));
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::{
        Foundation::{POINT, SIZE},
        Graphics::Gdi::{
            CreateCompatibleDC, DeleteDC, GetTextExtentPoint32W, HDC, HGDIOBJ, SelectObject,
        },
        UI::{
            Controls::{COMBOBOXINFO, GetComboBoxInfo},
            HiDpi::GetSystemMetricsForDpi,
            Input::KeyboardAndMouse::EnableWindow,
        },
    };

    #[test]
    fn close_and_resize_are_deferred_while_a_callback_is_active() {
        GATE.with(|gate| {
            gate.set(DispatchGate {
                active: true,
                ..DispatchGate::default()
            })
        });
        assert_eq!(unsafe { window_proc(null_mut(), WM_CLOSE, 0, 0) }, 0);
        assert_eq!(unsafe { window_proc(null_mut(), WM_SIZE, 0, 0) }, 0);
        GATE.with(|gate| {
            assert!(gate.get().close);
            assert!(gate.get().resize);
            gate.set(DispatchGate::default());
        });
    }

    struct NativeTestResources {
        hwnd: HWND,
        dc: HDC,
        font: HFONT,
        old_font: HGDIOBJ,
    }

    impl Drop for NativeTestResources {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.hwnd);
                SelectObject(self.dc, self.old_font);
                DeleteDC(self.dc);
                DeleteObject(self.font as _);
            }
        }
    }

    fn native_rect(view: &View, id: i32) -> RECT {
        unsafe {
            let mut rect: RECT = zeroed();
            assert_ne!(GetWindowRect(view.get(id), &mut rect), 0);
            let mut top_left = POINT {
                x: rect.left,
                y: rect.top,
            };
            assert_ne!(ScreenToClient(view.hwnd, &mut top_left), 0);
            RECT {
                left: top_left.x,
                top: top_left.y,
                right: top_left.x + rect.right - rect.left,
                bottom: top_left.y + rect.bottom - rect.top,
            }
        }
    }

    fn text_extent(dc: HDC, text: &str) -> SIZE {
        let text = wide(text);
        let mut extent = SIZE { cx: 0, cy: 0 };
        unsafe {
            assert_ne!(
                GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32 - 1, &mut extent),
                0
            );
        }
        extent
    }

    #[test]
    fn native_settings_open_in_view_while_capture_fields_are_locked() {
        let instance = unsafe { GetModuleHandleW(null()) };
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                390,
                280,
                null_mut(),
                null_mut(),
                instance,
                null(),
            )
        };
        struct HiddenWindow(HWND);
        impl Drop for HiddenWindow {
            fn drop(&mut self) {
                unsafe { DestroyWindow(self.0) };
            }
        }
        assert!(!hwnd.is_null());
        let _window = HiddenWindow(hwnd);
        let mut state = Window {
            view: create_view(hwnd, instance).unwrap(),
            controller: Controller::new(),
            scroll: 0,
            content: 0,
            font: null_mut(),
            heading_font: null_mut(),
            tray: false,
            programs_open: false,
            recent_open: false,
            recording_open: false,
            transcription_open: false,
            recording_controls: None,
            transcription_controls: None,
        };
        unsafe {
            for id in [PAUSE, MARK, STOP_BOTH] {
                EnableWindow(state.view.get(id), 1);
            }
            for id in [
                PROFILE,
                LOAD_LIVE_CONFIG,
                LIVE_ENDPOINT,
                LIVE_OPEN_TEXT,
                LIVE_OPEN_FOLDER,
                LIVE_CANCEL_PENDING,
            ] {
                EnableWindow(state.view.get(id), 0);
            }
            state.view.set_visible(LIVE_PREVIEW, true);
            state.refresh_view(None);
        }
        let height = 280;
        for (toggle, heading, field) in [
            (RECORDING_SETTINGS, RECORDING_SETTINGS_SECTION, PROFILE),
            (
                TRANSCRIPTION_SETTINGS,
                TRANSCRIPTION_SETTINGS_SECTION,
                LIVE_ENDPOINT,
            ),
        ] {
            let bounds = native_rect(&state.view, toggle);
            unsafe {
                state.scroll_to(focus_scroll(
                    state.scroll,
                    bounds.top,
                    bounds.bottom,
                    height,
                ))
            };
            assert!(native_rect(&state.view, toggle).bottom <= height);
            assert_ne!(unsafe { IsWindowEnabled(state.view.get(toggle)) }, 0);
            assert!(!state.view.visible(heading));
            unsafe { state.command(toggle, CLICKED) };
            assert!(state.view.visible(heading));
            let check_visible = |state: &Window| {
                let top = native_rect(&state.view, heading).top;
                let bottom = native_rect(&state.view, field).bottom;
                assert!(
                    top >= 0 && bottom <= height,
                    "panel {heading} is at {top}..{bottom} in a {height}px viewport"
                );
            };
            check_visible(&state);
            assert_eq!(unsafe { IsWindowEnabled(state.view.get(field)) }, 0);
            unsafe {
                EnableWindow(state.view.get(LIVE_OPEN_TEXT), 1);
                state.refresh_view(None);
            }
            check_visible(&state);
            unsafe { state.command(toggle, CLICKED) };
            assert!(!state.view.visible(heading));
            let bounds = native_rect(&state.view, toggle);
            assert!(bounds.top >= 0 && bounds.bottom <= height);
            unsafe {
                EnableWindow(state.view.get(LIVE_OPEN_TEXT), 0);
                state.refresh_view(None);
            }
        }
    }

    #[test]
    fn native_startup_config_populates_hidden_controls_and_keeps_them_on_error() {
        use audio_capture::transcription_config::{FILENAME, TranscriptionConfig};
        use std::{fs, path::PathBuf};

        struct Cleanup(HWND, PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                unsafe { DestroyWindow(self.0) };
                let _ = fs::remove_file(self.1.join(FILENAME));
                let _ = fs::remove_dir(&self.1);
            }
        }

        let folder = std::env::temp_dir().join(audio_capture::live_wire::new_interval_id());
        fs::create_dir(&folder).unwrap();
        let instance = unsafe { GetModuleHandleW(null()) };
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                390,
                440,
                null_mut(),
                null_mut(),
                instance,
                null(),
            )
        };
        let _cleanup = Cleanup(hwnd, folder.clone());
        assert!(!hwnd.is_null());
        let view = create_view(hwnd, instance).unwrap();
        for id in [LOAD_LIVE_CONFIG, SAVE_LIVE_CONFIG] {
            assert!(!view.visible(id));
        }
        view.set_panel(TRANSCRIPTION_CONTROLS, true);
        native_layout::layout(&view, 390, 440, 96, 0);
        let load = native_rect(&view, LOAD_LIVE_CONFIG);
        let save = native_rect(&view, SAVE_LIVE_CONFIG);
        assert!(load.left < load.right && load.right < save.left && save.right <= 390);
        assert_eq!(
            unsafe { GetNextDlgTabItem(hwnd, view.get(LOAD_LIVE_CONFIG), 0) },
            view.get(SAVE_LIVE_CONFIG)
        );
        assert_eq!(
            unsafe { GetNextDlgTabItem(hwnd, view.get(SAVE_LIVE_CONFIG), 0) },
            view.get(LIVE_ENDPOINT)
        );

        let mut app = Controller::new();
        let exe = folder.join("audio-capture.exe");
        app.load_startup_transcription_config(&view, &exe);
        assert_eq!(view.read_caption(LIVE_ENDPOINT), [0; 32]);
        let path = folder.join(FILENAME);
        let config = TranscriptionConfig {
            endpoint: "http://127.0.0.1:8123/v1".into(),
            model: "test-model".into(),
            api_key_env: "TEST_TRANSCRIPTION_API_KEY".into(),
            transcript_output_folder: folder.join("transcripts"),
            request_segment_timestamps: true,
            speaker_adapter_config: folder.join("speaker.json"),
            ..TranscriptionConfig::default()
        };
        config.save(&path).unwrap();
        view.caption(FOLDER, "Keep this recording folder");
        app.load_startup_transcription_config(&view, &exe);
        let assert_text = |id, expected: &str| {
            let mut text = vec![0; expected.encode_utf16().count() + 2];
            let len = unsafe { GetWindowTextW(view.get(id), text.as_mut_ptr(), text.len() as i32) };
            assert_eq!(String::from_utf16_lossy(&text[..len as usize]), expected);
        };
        assert_text(
            LIVE_STATUS,
            "Open Transcription settings to load a config or enter an endpoint and model.",
        );
        app.command(&view, LIVE_MODEL, EN_KILLFOCUS as u16);
        assert_text(
            LIVE_STATUS,
            "Loaded transcription config from transcription.json",
        );
        for (id, expected) in [
            (LIVE_ENDPOINT, config.endpoint.as_str()),
            (LIVE_MODEL, config.model.as_str()),
            (LIVE_KEY_ENV, config.api_key_env.as_str()),
            (
                LIVE_OUTPUT_DIR,
                config.transcript_output_folder.to_str().unwrap(),
            ),
            (
                LIVE_SPEAKER_CONFIG,
                config.speaker_adapter_config.to_str().unwrap(),
            ),
            (FOLDER, "Keep this recording folder"),
        ] {
            assert_text(id, expected);
        }
        let checked = |id| unsafe { SendMessageW(view.get(id), BM_GETCHECK, 0, 0) == 1 };
        assert!(checked(LIVE_TIMESTAMPS));
        assert!(!checked(LIVE_GATE));
        assert!(!checked(LIVE_SPEAKERS));
        fs::write(&path, b"{\"version\":999}").unwrap();
        app.load_startup_transcription_config(&view, &exe);
        assert!(view.show_live_settings.get());
        assert_text(LIVE_MODEL, &config.model);
        assert_eq!(fs::read(path).unwrap(), b"{\"version\":999}");
        assert!(app.close_ready());

        app.command(&view, LIVE_MODEL, EN_CHANGE as u16);
        assert_text(LIVE_STATUS, "Transcription settings configured.");
        view.caption(LIVE_ENDPOINT, "invalid-url");
        app.command(&view, LIVE_ENDPOINT, EN_CHANGE as u16);
        assert_text(
            LIVE_STATUS,
            "Check Transcription settings. Endpoint must use HTTP or HTTPS",
        );
        view.caption(LIVE_ENDPOINT, "");
        app.command(&view, LIVE_ENDPOINT, EN_CHANGE as u16);
        assert_text(
            LIVE_STATUS,
            "Open Transcription settings to load a config or enter an endpoint and model.",
        );
    }

    #[test]
    fn native_codec_text_and_activity_focus_fit_at_target_dpi() {
        for dpi in [96, 120, 144, 192] {
            let pixels = |logical| (logical * dpi as i32 + 48) / 96;
            unsafe {
                // Measure the target font and chrome through Win32. Keep the
                // fixture hidden and leave the physical monitor scale alone.
                let instance = GetModuleHandleW(null());
                let hwnd = CreateWindowExW(
                    0,
                    wide("STATIC").as_ptr(),
                    null(),
                    WS_POPUP,
                    0,
                    0,
                    pixels(660),
                    pixels(440),
                    null_mut(),
                    null_mut(),
                    instance,
                    null(),
                );
                let dc = CreateCompatibleDC(null_mut());
                let font = ui_font(dpi, 9, 400);
                let resources = NativeTestResources {
                    hwnd,
                    dc,
                    font,
                    old_font: SelectObject(dc, font as _),
                };
                assert!(!resources.hwnd.is_null() && !resources.old_font.is_null());
                let view = create_view(hwnd, instance).expect("native test controls");
                for handle in view.controls.values() {
                    SendMessageW(*handle, WM_SETFONT, font as usize, 0);
                }
                view.set_panel(RECORDING_CONTROLS, true);
                for (id, caption) in [
                    (START, "Stop recording"),
                    (PAUSE, "Pause recording"),
                    (MARK, "Add marker"),
                    (LIVE_TOGGLE, "Stop transcription"),
                    (STOP_BOTH, "Stop both"),
                ] {
                    view.caption(id, caption);
                    EnableWindow(view.get(id), 1);
                    view.set_visible(id, true);
                }
                view.set_visible(LIVE_PREVIEW, true);
                view.set_visible(LIVE_PREVIEW_LABEL, true);

                for logical_width in [390, 660] {
                    let width = pixels(logical_width);
                    let height = pixels(440);
                    assert_ne!(
                        SetWindowPos(
                            hwnd,
                            null_mut(),
                            0,
                            0,
                            width,
                            height,
                            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE
                        ),
                        0
                    );
                    let content = native_layout::layout(&view, width, height, dpi, 0);

                    let combo_chrome = GetSystemMetricsForDpi(SM_CXVSCROLL, dpi)
                        + 2 * GetSystemMetricsForDpi(SM_CXEDGE, dpi)
                        + pixels(6);
                    for (label, control, text, other) in [
                        (BITRATE_LABEL, BITRATE, "256", SAMPLE),
                        (SAMPLE_LABEL, SAMPLE, "Auto (48000 Hz)", BITRATE),
                    ] {
                        let native = view.get(control);
                        SendMessageW(native, CB_RESETCONTENT, 0, 0);
                        assert_eq!(
                            SendMessageW(native, CB_ADDSTRING, 0, wide(text).as_ptr() as isize),
                            0
                        );
                        assert_eq!(SendMessageW(native, CB_SETCURSEL, 0, 0), 0);
                        let mut combo: COMBOBOXINFO = zeroed();
                        combo.cbSize = size_of::<COMBOBOXINFO>() as u32;
                        assert_ne!(GetComboBoxInfo(native, &mut combo), 0);
                        let label_extent = text_extent(
                            dc,
                            &String::from_utf16_lossy(
                                &view
                                    .read_caption(label)
                                    .into_iter()
                                    .take_while(|unit| *unit != 0)
                                    .collect::<Vec<_>>(),
                            ),
                        );
                        let label_rect = native_rect(&view, label);
                        assert!(label_extent.cx <= label_rect.right - label_rect.left);
                        assert!(label_extent.cy <= label_rect.bottom - label_rect.top);
                        let bounds = native_rect(&view, control);
                        let actual_item_width = combo.rcItem.right - combo.rcItem.left;
                        let scaled_item_width = bounds.right - bounds.left - combo_chrome;
                        let extent = text_extent(dc, text);
                        assert!(
                            extent.cx <= actual_item_width.min(scaled_item_width),
                            "{logical_width} logical px at {dpi} DPI, {text}, width {}",
                            actual_item_width.min(scaled_item_width)
                        );
                        assert!(extent.cy <= combo.rcItem.bottom - combo.rcItem.top);
                        EnableWindow(view.get(other), 0);
                        EnableWindow(native, 1);
                        assert_eq!(GetNextDlgTabItem(hwnd, view.get(CHANNEL), 0), native);
                    }
                    for selector in [FORMAT, CHANNEL, BITRATE, SAMPLE] {
                        EnableWindow(view.get(selector), 0);
                    }
                    let entry = view.get(SELECT_PROGRAMS);
                    let tab_order: Vec<_> = std::iter::successors(Some(entry), |control| {
                        let next = GetNextDlgTabItem(hwnd, *control, 0);
                        (next != entry && !next.is_null()).then_some(next)
                    })
                    .take(view.controls.len())
                    .collect();
                    for (id, caption) in [
                        (START, "Stop recording"),
                        (LIVE_TOGGLE, "Stop transcription"),
                        (STOP_BOTH, "Stop both"),
                    ] {
                        assert!(tab_order.contains(&view.get(id)));
                        let before = native_rect(&view, id);
                        let scroll = focus_scroll(0, before.top, before.bottom, height)
                            .clamp(0, (content - height).max(0));
                        native_layout::layout(&view, width, height, dpi, scroll);
                        let bounds = native_rect(&view, id);
                        let extent = text_extent(dc, caption);
                        assert!(bounds.left >= 0 && bounds.right <= width);
                        assert!(bounds.top >= 0 && bounds.bottom <= height);
                        assert!(extent.cx + pixels(12) <= bounds.right - bounds.left);
                        assert!(extent.cy + pixels(4) <= bounds.bottom - bounds.top);
                        native_layout::layout(&view, width, height, dpi, 0);
                    }
                    EnableWindow(view.get(FORMAT), 1);
                    EnableWindow(view.get(CHANNEL), 1);
                }
            }
        }
    }
}

struct Control {
    id: i32,
    class: &'static str,
    label: &'static str,
    style: u32,
}

fn create_view(hwnd: HWND, instance: HINSTANCE) -> Result<View, String> {
    unsafe {
        let label = |id, label| Control {
            id,
            class: "STATIC",
            label,
            style: 0,
        };
        let button = |id, label| Control {
            id,
            class: "BUTTON",
            label,
            style: WS_TABSTOP | BS_PUSHBUTTON as u32,
        };
        let check = |id, label| Control {
            id,
            class: "BUTTON",
            label,
            style: WS_TABSTOP | BS_AUTOCHECKBOX as u32 | BS_MULTILINE as u32,
        };
        let combo = |id| Control {
            id,
            class: "COMBOBOX",
            label: "",
            style: WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
        };
        let edit = |id| Control {
            id,
            class: "EDIT",
            label: "",
            style: WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
        };
        let readout = |id, label| Control {
            id,
            class: "EDIT",
            label,
            style: WS_TABSTOP
                | WS_BORDER
                | WS_VSCROLL
                | ES_MULTILINE as u32
                | ES_READONLY as u32
                | ES_AUTOVSCROLL as u32,
        };
        // Native dialog navigation follows child creation order. Keep the visual
        // order in native_layout consistent with this list.
        let items = [
            label(SOURCES_SECTION, "Sources"),
            button(SELECT_PROGRAMS, "Select programs…"),
            button(RECENT_OPTIONS, "Manage recent"),
            label(RECENT_LABEL, "Recent programs"),
            combo(RECENT_PICKER),
            button(USE_RECENT, "Use recent"),
            check(AUTO_SELECT_RECENT, "Auto-select recent"),
            button(FORGET_RECENT, "Forget recent"),
            button(CLEAR_RECENT, "Clear recent"),
            label(MIC_LABEL, "&Microphone"),
            combo(MIC),
            Control {
                id: SOURCE_SUMMARY,
                class: "STATIC",
                label: "Choose a program or microphone.",
                style: STATIC_LITERAL | STATIC_ELLIPSIS,
            },
            label(PROCESS_LABEL, "&Programs (Ctrl-click to select multiple)"),
            Control {
                id: PROCESS_LIST,
                class: "LISTBOX",
                label: "",
                style: WS_TABSTOP
                    | WS_BORDER
                    | WS_VSCROLL
                    | WS_HSCROLL
                    | LBS_EXTENDEDSEL as u32
                    | LBS_NOTIFY as u32
                    | LBS_NOINTEGRALHEIGHT as u32,
            },
            button(REFRESH, "Refresh"),
            check(FILTER, "Show only programs with active audio"),
            check(SYSTEM, "Capture all system audio"),
            label(RECORDING_SECTION, "Recording"),
            button(START, "Start recording"),
            button(PAUSE, "Pause"),
            button(MARK, "Add marker"),
            button(RECORDING_SETTINGS, "Recording settings"),
            label(RECORDING_STATUS_LABEL, "Recording status"),
            readout(STATUS, "Ready"),
            label(LIVE_SECTION, "Live transcription"),
            button(LIVE_TOGGLE, "Start transcription"),
            button(STOP_BOTH, "Stop both"),
            button(TRANSCRIPTION_SETTINGS, "Transcription settings"),
            label(LIVE_STATUS_LABEL, "Transcription status"),
            readout(LIVE_STATUS, TRANSCRIPTION_SETUP_NEEDED),
            button(LIVE_OPEN_TEXT, "Open transcript"),
            button(LIVE_OPEN_FOLDER, "Open folder"),
            button(LIVE_CANCEL_PENDING, "Cancel pending requests"),
            label(LIVE_PREVIEW_LABEL, "Transcript preview"),
            readout(LIVE_PREVIEW, ""),
            label(BATCH_SECTION, "Completed files"),
            button(TRANSCRIBE_FILES, "Transcribe files…"),
            button(BATCH_MANAGE, "Current job"),
            label(LOCAL_INPUT_LABEL, "MP4 &file"),
            edit(LOCAL_INPUT),
            button(LOCAL_INPUT_BROWSE, "Browse…"),
            label(LOCAL_OUTPUT_LABEL, "Save transcripts in"),
            edit(LOCAL_OUTPUT),
            button(LOCAL_OUTPUT_BROWSE, "Browse…"),
            button(LOCAL_PREVIEW, "Preview local job"),
            button(LOCAL_APPROVE, "Start local transcript"),
            button(LOCAL_CANCEL, "Cancel job"),
            label(LOCAL_STATUS_LABEL, "Local job status"),
            readout(LOCAL_STATUS, "Choose an MP4 file and an output folder."),
            label(LOCAL_REVIEW_LABEL, "Timed transcript"),
            readout(LOCAL_REVIEW, ""),
            button(LOCAL_PREVIOUS, "Previous page"),
            button(LOCAL_NEXT, "Next page"),
            label(LOCAL_MODEL_LABEL, "Model pack folder"),
            edit(LOCAL_MODEL),
            button(LOCAL_MODEL_BROWSE, "Browse…"),
            button(LOCAL_IMPORT, "Import base.en pack"),
            button(ENDPOINT_FILES, "Use an endpoint…"),
            button(ENDPOINT_QUEUE, "Endpoint queue…"),
            label(ENDPOINT_REPLY_LABEL, "Endpoint reply"),
            edit(ENDPOINT_REPLY),
            button(ENDPOINT_SEND, "Send reply"),
            button(ENDPOINT_CLOSE, "Close session"),
            label(RECORDING_SETTINGS_SECTION, "Recording settings"),
            label(PROFILE_LABEL, "&Recording profile"),
            combo(PROFILE),
            label(PROFILE_HELP, ""),
            label(FORMAT_LABEL, "&Format"),
            combo(FORMAT),
            label(CHANNEL_LABEL, "Channels"),
            combo(CHANNEL),
            label(BITRATE_LABEL, "Opus bitrate (kbps)"),
            combo(BITRATE),
            label(SAMPLE_LABEL, "Sample rate (Hz)"),
            combo(SAMPLE),
            label(PRESET_LABEL, "Saved setup"),
            Control {
                id: PRESET,
                class: "COMBOBOX",
                label: "",
                style: WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWN as u32 | CBS_AUTOHSCROLL as u32,
            },
            button(SAVE, "Save setup"),
            button(LOAD, "Load setup"),
            button(DELETE, "Delete setup"),
            label(MONITOR_LABEL, "Monitor output"),
            combo(MONITOR),
            check(MONITOR_ONLY, "Monitor only"),
            label(APP_GAIN_LABEL, "Program volume"),
            combo(APP_GAIN),
            label(MIC_GAIN_LABEL, "Microphone volume"),
            combo(MIC_GAIN),
            label(MODE_LABEL, "Recording mode"),
            combo(MODE),
            check(SILENCE, "Skip silence"),
            label(FOLDER_LABEL, "Output folder"),
            edit(FOLDER),
            button(BROWSE, "Browse…"),
            label(TRANSCRIPTION_SETTINGS_SECTION, "Transcription settings"),
            button(LOAD_LIVE_CONFIG, "Load config…"),
            button(SAVE_LIVE_CONFIG, "Save config…"),
            label(LIVE_ENDPOINT_LABEL, "Endpoint"),
            edit(LIVE_ENDPOINT),
            label(LIVE_MODEL_LABEL, "Model"),
            edit(LIVE_MODEL),
            label(LIVE_KEY_ENV_LABEL, "API key environment variable"),
            edit(LIVE_KEY_ENV),
            label(LIVE_OUTPUT_DIR_LABEL, "Transcript output folder"),
            edit(LIVE_OUTPUT_DIR),
            button(LIVE_BROWSE, "Browse…"),
            check(
                LIVE_GATE,
                if QUALIFIED_LIVE_GATE {
                    "Speech gating"
                } else {
                    "Speech gating (evaluation only)"
                },
            ),
            check(LIVE_TIMESTAMPS, "Request segment timestamps"),
            check(
                LIVE_SPEAKERS,
                if QUALIFIED_LIVE_SPEAKERS {
                    "Speaker labels"
                } else {
                    "Speaker labels (evaluation only)"
                },
            ),
            label(LIVE_SPEAKER_CONFIG_LABEL, "Speaker adapter config"),
            edit(LIVE_SPEAKER_CONFIG),
            label(LEVELS_LABEL, "Source levels"),
            readout(LEVELS, "Source levels appear during capture."),
            button(STOP, "Stop recording"),
        ];
        let mut view = View {
            hwnd,
            controls: HashMap::with_capacity(items.len()),
            layout_dirty: Cell::new(true),
            show_live_settings: Cell::new(false),
        };
        for item in items {
            let disabled_until_active = [
                PAUSE,
                MARK,
                STOP_BOTH,
                LIVE_CANCEL_PENDING,
                LIVE_OPEN_TEXT,
                LIVE_OPEN_FOLDER,
            ]
            .contains(&item.id);
            let h = CreateWindowExW(
                0,
                wide(item.class).as_ptr(),
                wide(item.label).as_ptr(),
                WS_CHILD
                    | WS_VISIBLE
                    | item.style
                    | if disabled_until_active {
                        WS_DISABLED
                    } else {
                        0
                    },
                0,
                0,
                1,
                1,
                hwnd,
                item.id as usize as _,
                instance,
                null(),
            );
            if h.is_null() {
                return Err(format!(
                    "Cannot create control {}: {}",
                    item.id,
                    std::io::Error::last_os_error()
                ));
            }
            view.controls.insert(item.id, h);
        }
        for panel in [
            LOCAL_PANEL,
            PROGRAM_CONTROLS,
            RECENT_CONTROLS,
            RECORDING_CONTROLS,
            TRANSCRIPTION_CONTROLS,
        ] {
            view.set_panel(panel, false);
        }
        view.set_visible(STOP, false);
        view.set_visible(LIVE_PREVIEW, false);
        view.set_visible(LIVE_PREVIEW_LABEL, false);
        for (id, limit, hint) in [
            (LIVE_ENDPOINT, MAX_ENDPOINT_UNITS, "https://example.com/v1"),
            (LOCAL_INPUT, MAX_PATH_UNITS, "Choose a local MP4"),
            (LOCAL_OUTPUT, MAX_PATH_UNITS, "Choose an existing folder"),
            (
                LOCAL_MODEL,
                MAX_PATH_UNITS,
                "Extract the base.en model pack and select its folder",
            ),
            (ENDPOINT_REPLY, MAX_PATH_UNITS, "Answer the question above"),
            (LIVE_MODEL, MAX_FIELD_UNITS, "Model from your endpoint"),
            (LIVE_KEY_ENV, MAX_FIELD_UNITS, "Optional"),
            (
                LIVE_OUTPUT_DIR,
                MAX_PATH_UNITS,
                "Use recording output folder",
            ),
            (
                LIVE_SPEAKER_CONFIG,
                MAX_PATH_UNITS,
                "Optional local adapter configuration file",
            ),
        ] {
            SendMessageW(view.get(id), EM_LIMITTEXT, limit, 0);
            SendMessageW(
                view.get(id),
                EM_SETCUEBANNER,
                0,
                wide(hint).as_ptr() as isize,
            );
        }
        Ok(view)
    }
}

pub fn run() -> Result<(), String> {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(null());
        let class = wide("AudioCaptureRustWindow");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: (COLOR_WINDOW + 1) as _,
            lpszClassName: class.as_ptr(),
            ..zeroed()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(format!(
                "Cannot register window: {}",
                std::io::Error::last_os_error()
            ));
        }
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("Audio Capture").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VSCROLL | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1040,
            808,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return Err(format!(
                "Cannot create window: {}",
                std::io::Error::last_os_error()
            ));
        }
        let view = match create_view(hwnd, instance) {
            Ok(view) => view,
            Err(error) => {
                DestroyWindow(hwnd);
                return Err(error);
            }
        };
        let mut state = Box::new(Window {
            view,
            controller: Controller::new(),
            scroll: 0,
            content: 0,
            font: null_mut(),
            heading_font: null_mut(),
            tray: false,
            programs_open: false,
            recent_open: false,
            recording_open: false,
            transcription_open: false,
            recording_controls: None,
            transcription_controls: None,
        });
        state.update_font();
        state.controller.initialize(&state.view);
        let ptr = Box::into_raw(state);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        guarded(hwnd, || {
            (*ptr).tray = tray(hwnd, NIM_ADD);
            (*ptr).refresh_view(None);
        });
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        let mut msg: MSG = zeroed();
        let mut previous_focus = null_mut();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Window;
            let focus = GetFocus();
            if !ptr.is_null() && focus != previous_focus {
                guarded(hwnd, || (*ptr).ensure_focus_visible());
            }
            previous_focus = focus;
        }
        Ok(())
    }
}
