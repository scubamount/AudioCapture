use std::ptr::null_mut;
use std::sync::OnceLock;
use windows::core::w;
use windows_sys::Win32::Foundation::FreeLibrary;
use windows_sys::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};

pub(super) fn require_media_foundation() -> Result<(), String> {
    static AVAILABLE: OnceLock<Result<(), String>> = OnceLock::new();
    AVAILABLE.get_or_init(load_media_foundation).clone()
}

fn load_media_foundation() -> Result<(), String> {
    let missing = || {
        "Video transcription requires Windows Media Foundation. On Windows N, install the Media Feature Pack."
            .to_owned()
    };
    // Keep successful references for the worker's lifetime. The linker helper
    // reuses the loaded system DLLs when the decoder calls their first exports.
    unsafe {
        let platform = LoadLibraryExW(
            w!("mfplat.dll").as_ptr(),
            null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
        if platform.is_null() {
            return Err(missing());
        }
        let reader = LoadLibraryExW(
            w!("mfreadwrite.dll").as_ptr(),
            null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
        if reader.is_null() {
            FreeLibrary(platform);
            return Err(missing());
        }
    }
    Ok(())
}
