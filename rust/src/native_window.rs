//! Native Windows controls and message dispatch for the recorder.
use crate::{native_controller::Controller, native_layout};
use std::{
    collections::HashMap,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{COLOR_WINDOW, CreateFontW, DeleteObject, HFONT, ScreenToClient, UpdateWindow},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::SetScrollInfo,
        HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
            SetProcessDpiAwarenessContext,
        },
        Input::KeyboardAndMouse::GetFocus,
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
pub const PROFILE_WIDTH: i32 = 310;

const TRAY_MESSAGE: u32 = WM_APP + 14;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

pub struct View {
    pub hwnd: HWND,
    controls: HashMap<i32, HWND>,
}

impl View {
    pub fn get(&self, id: i32) -> HWND {
        self.controls.get(&id).copied().unwrap_or(null_mut())
    }
    pub fn place(&self, id: i32, x: i32, y: i32, w: i32, h: i32) {
        let combo = [
            PROFILE, PRESET, FORMAT, BITRATE, SAMPLE, CHANNEL, MIC, MONITOR, APP_GAIN, MIC_GAIN,
            MODE,
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
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    pub fn visible(&self, id: i32) -> bool {
        unsafe { (GetWindowLongPtrW(self.get(id), GWL_STYLE) as u32 & WS_VISIBLE) != 0 }
    }
}

struct Window {
    view: View,
    controller: Controller,
    scroll: i32,
    content: i32,
    font: HFONT,
    tray: bool,
}

impl Window {
    unsafe fn update_font(&mut self) {
        let dpi = unsafe { GetDpiForWindow(self.view.hwnd) }.max(96);
        let font = unsafe {
            CreateFontW(
                -((9 * dpi as i32 + 36) / 72),
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                1,
                0,
                0,
                5,
                0,
                wide("Segoe UI").as_ptr(),
            )
        };
        if !font.is_null() {
            for hwnd in self.view.controls.values() {
                unsafe {
                    SendMessageW(*hwnd, WM_SETFONT, font as usize, 1);
                }
            }
            if !self.font.is_null() {
                unsafe {
                    DeleteObject(self.font as _);
                }
            }
            self.font = font;
        }
        let profile_width = (PROFILE_WIDTH as usize * dpi as usize + 48) / 96;
        unsafe {
            SendMessageW(self.view.get(PROFILE), CB_SETDROPPEDWIDTH, profile_width, 0);
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
        }
    }
    unsafe fn scroll_to(&mut self, value: i32) {
        self.scroll = value;
        unsafe {
            self.layout();
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
        let mut bounds: RECT = unsafe { zeroed() };
        let mut client: RECT = unsafe { zeroed() };
        unsafe {
            GetWindowRect(focus, &mut bounds);
            GetClientRect(self.view.hwnd, &mut client);
        }
        let mut point = windows_sys::Win32::Foundation::POINT {
            x: bounds.left,
            y: bounds.top,
        };
        unsafe {
            ScreenToClient(self.view.hwnd, &mut point);
        }
        let bottom = point.y + bounds.bottom - bounds.top;
        if point.y < 0 {
            unsafe {
                self.scroll_to(self.scroll + point.y - 8);
            }
        } else if bottom > client.bottom {
            unsafe {
                self.scroll_to(self.scroll + bottom - client.bottom + 8);
            }
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
                    (*ptr).controller.command(&(*ptr).view, id, notification);
                    (*ptr).layout();
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
                guarded(hwnd, || (*ptr).controller.tick(&(*ptr).view));
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
                guarded(hwnd, || {
                    close_result = (*ptr).controller.shutdown(&(*ptr).view);
                });
                match close_result {
                    Ok(()) => {
                        DestroyWindow(hwnd);
                    }
                    Err(error) => {
                        MessageBoxW(
                            hwnd,
                            wide(&error).as_ptr(),
                            wide("Could not save recording or settings").as_ptr(),
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
}

struct Control {
    id: i32,
    class: &'static str,
    label: &'static str,
    style: u32,
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
            WS_OVERLAPPEDWINDOW | WS_VSCROLL,
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
        let label = 0;
        let button = WS_TABSTOP | BS_PUSHBUTTON as u32;
        let check = WS_TABSTOP | BS_AUTOCHECKBOX as u32 | BS_MULTILINE as u32;
        let combo = WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32;
        let items = [
            Control {
                id: PROFILE_LABEL,
                class: "STATIC",
                label: "&Recording profile",
                style: label,
            },
            Control {
                id: PROFILE,
                class: "COMBOBOX",
                label: "",
                style: combo,
            },
            Control {
                id: PROFILE_HELP,
                class: "STATIC",
                label: "",
                style: label,
            },
            Control {
                id: FORMAT_LABEL,
                class: "STATIC",
                label: "Format",
                style: label,
            },
            Control {
                id: FORMAT,
                class: "COMBOBOX",
                label: "",
                style: combo,
            },
            Control {
                id: CHANNEL_LABEL,
                class: "STATIC",
                label: "Channels",
                style: label,
            },
            Control {
                id: CHANNEL,
                class: "COMBOBOX",
                label: "",
                style: combo,
            },
            Control {
                id: BITRATE_LABEL,
                class: "STATIC",
                label: "Opus bitrate (kbps)",
                style: label,
            },
            Control {
                id: BITRATE,
                class: "COMBOBOX",
                label: "",
                style: combo,
            },
            Control {
                id: SAMPLE_LABEL,
                class: "STATIC",
                label: "Sample rate (Hz)",
                style: label,
            },
            Control {
                id: SAMPLE,
                class: "COMBOBOX",
                label: "",
                style: combo,
            },
            Control {
                id: START,
                class: "BUTTON",
                label: "Start recording",
                style: button,
            },
            Control {
                id: PAUSE,
                class: "BUTTON",
                label: "Pause",
                style: button,
            },
            Control {
                id: STOP,
                class: "BUTTON",
                label: "Stop",
                style: button,
            },
            Control {
                id: MARK,
                class: "BUTTON",
                label: "Add marker",
                style: button,
            },
            Control {
                id: TRANSCRIBE_FILES,
                class: "BUTTON",
                label: "Transcribe files…",
                style: button,
            },
            Control {
                id: PRESET_LABEL,
                class: "STATIC",
                label: "Saved setup",
                style: label,
            },
            Control {
                id: PRESET,
                class: "COMBOBOX",
                label: "",
                style: WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWN as u32 | CBS_AUTOHSCROLL as u32,
            },
            Control {
                id: SAVE,
                class: "BUTTON",
                label: "Save setup",
                style: button,
            },
            Control {
                id: LOAD,
                class: "BUTTON",
                label: "Load setup",
                style: button,
            },
            Control {
                id: DELETE,
                class: "BUTTON",
                label: "Delete setup",
                style: button,
            },
            Control {
                id: PROCESS_LABEL,
                class: "STATIC",
                label: "Programs (Ctrl-click to select multiple)",
                style: label,
            },
            Control {
                id: PROCESS_LIST,
                class: "LISTBOX",
                label: "",
                style: WS_TABSTOP
                    | WS_BORDER
                    | WS_VSCROLL
                    | LBS_EXTENDEDSEL as u32
                    | LBS_NOTIFY as u32
                    | LBS_NOINTEGRALHEIGHT as u32,
            },
            Control {
                id: REFRESH,
                class: "BUTTON",
                label: "Refresh",
                style: button,
            },
            Control {
                id: FILTER,
                class: "BUTTON",
                label: "Show only programs with active audio",
                style: check,
            },
            Control {
                id: SYSTEM,
                class: "BUTTON",
                label: "Capture all system audio",
                style: check,
            },
            Control {
                id: MIC_LABEL,
                class: "STATIC",
                label: "Microphone",
                style: label,
            },
            Control {
                id: MIC,
                class: "COMBOBOX",
                label: "",
                style: combo,
            },
            Control {
                id: MONITOR_LABEL,
                class: "STATIC",
                label: "Monitor output",
                style: label,
            },
            Control {
                id: MONITOR,
                class: "COMBOBOX",
                label: "",
                style: combo,
            },
            Control {
                id: MONITOR_ONLY,
                class: "BUTTON",
                label: "Monitor only",
                style: check,
            },
            Control {
                id: APP_GAIN_LABEL,
                class: "STATIC",
                label: "Program volume",
                style: label,
            },
            Control {
                id: APP_GAIN,
                class: "COMBOBOX",
                label: "",
                style: combo,
            },
            Control {
                id: MIC_GAIN_LABEL,
                class: "STATIC",
                label: "Microphone volume",
                style: label,
            },
            Control {
                id: MIC_GAIN,
                class: "COMBOBOX",
                label: "",
                style: combo,
            },
            Control {
                id: MODE_LABEL,
                class: "STATIC",
                label: "Recording mode",
                style: label,
            },
            Control {
                id: MODE,
                class: "COMBOBOX",
                label: "",
                style: combo,
            },
            Control {
                id: SILENCE,
                class: "BUTTON",
                label: "Skip silence",
                style: check,
            },
            Control {
                id: FOLDER_LABEL,
                class: "STATIC",
                label: "Output folder",
                style: label,
            },
            Control {
                id: FOLDER,
                class: "EDIT",
                label: "",
                style: WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32,
            },
            Control {
                id: BROWSE,
                class: "BUTTON",
                label: "Browse…",
                style: button,
            },
            Control {
                id: LEVELS_LABEL,
                class: "STATIC",
                label: "Source levels",
                style: label,
            },
            Control {
                id: LEVELS,
                class: "EDIT",
                label: "Source levels appear while recording.",
                style: WS_TABSTOP
                    | WS_BORDER
                    | WS_VSCROLL
                    | ES_MULTILINE as u32
                    | ES_READONLY as u32
                    | ES_AUTOVSCROLL as u32,
            },
            Control {
                id: STATUS,
                class: "EDIT",
                label: "Ready",
                style: WS_TABSTOP
                    | WS_BORDER
                    | WS_VSCROLL
                    | ES_MULTILINE as u32
                    | ES_READONLY as u32
                    | ES_AUTOVSCROLL as u32,
            },
        ];
        let mut view = View {
            hwnd,
            controls: HashMap::new(),
        };
        for item in items {
            let h = CreateWindowExW(
                0,
                wide(item.class).as_ptr(),
                wide(item.label).as_ptr(),
                WS_CHILD | WS_VISIBLE | item.style,
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
                DestroyWindow(hwnd);
                return Err(format!(
                    "Cannot create control {}: {}",
                    item.id,
                    std::io::Error::last_os_error()
                ));
            }
            view.controls.insert(item.id, h);
        }
        let mut state = Box::new(Window {
            view,
            controller: Controller::new(),
            scroll: 0,
            content: 0,
            font: null_mut(),
            tray: false,
        });
        state.update_font();
        state.controller.initialize(&state.view);
        let ptr = Box::into_raw(state);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        guarded(hwnd, || {
            (*ptr).tray = tray(hwnd, NIM_ADD);
            (*ptr).layout();
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
