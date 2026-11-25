#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod native_controller;
mod native_layout;
mod native_window;
mod worker_launch;

fn main() {
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
    }
}
