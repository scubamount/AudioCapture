//! Read-only COM stream over the file already locked and hashed by the job.

use std::ffi::c_void;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows_core::{HRESULT, Ref, Result, implement};

use super::mp4;

#[implement(IStream)]
struct FileStream {
    file: Arc<Mutex<File>>,
    length: u64,
    hidden: Arc<[u64]>,
    offset: Mutex<u64>,
}

pub(super) fn from_file(file: File, hidden: Vec<u64>) -> std::io::Result<IStream> {
    let length = file.metadata()?.len();
    Ok(FileStream {
        file: Arc::new(Mutex::new(file)),
        length,
        hidden: hidden.into(),
        offset: Mutex::new(0),
    }
    .into())
}

impl ISequentialStream_Impl for FileStream_Impl {
    fn Read(&self, buffer: *mut c_void, requested: u32, count: *mut u32) -> HRESULT {
        // COM permits a null count and an empty request. Its caller owns the output storage.
        if !count.is_null() {
            unsafe { count.write(0) };
        }
        if requested == 0 {
            return S_OK;
        }
        if buffer.is_null() {
            return E_POINTER;
        }
        let (Ok(mut offset), Ok(mut file)) = (self.offset.lock(), self.file.lock()) else {
            return STG_E_READFAULT;
        };
        let available = self.length.saturating_sub(*offset).min(requested.into()) as usize;
        if available == 0 {
            return S_FALSE;
        }
        if file.seek(SeekFrom::Start(*offset)).is_err() {
            return STG_E_SEEKERROR;
        }
        // The native buffer can be uninitialized. Initialize it before exposing it to Read.
        let bytes = unsafe {
            buffer.cast::<u8>().write_bytes(0, available);
            std::slice::from_raw_parts_mut(buffer.cast::<u8>(), available)
        };
        if file.read_exact(bytes).is_err() {
            return STG_E_READFAULT;
        }
        mp4::hide_tracks(bytes, *offset, &self.hidden);
        *offset += available as u64;
        if !count.is_null() {
            unsafe { count.write(available as u32) };
        }
        if available == requested as usize {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn Write(&self, _: *const c_void, _: u32, _: *mut u32) -> HRESULT {
        STG_E_ACCESSDENIED
    }
}

impl IStream_Impl for FileStream_Impl {
    fn Seek(&self, delta: i64, origin: STREAM_SEEK, result: *mut u64) -> Result<()> {
        let mut offset = self.offset.lock().map_err(|_| STG_E_SEEKERROR)?;
        let base = match origin {
            STREAM_SEEK_SET => 0,
            STREAM_SEEK_CUR => *offset,
            STREAM_SEEK_END => self.length,
            _ => return Err(STG_E_INVALIDFUNCTION.into()),
        };
        *offset = base.checked_add_signed(delta).ok_or(STG_E_SEEKERROR)?;
        if !result.is_null() {
            unsafe { result.write(*offset) };
        }
        Ok(())
    }

    fn Stat(&self, stat: *mut STATSTG, _: &STATFLAG) -> Result<()> {
        if stat.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            stat.write(STATSTG {
                r#type: STGTY_STREAM.0 as u32,
                cbSize: self.length,
                grfMode: STGM_READ,
                ..Default::default()
            });
        }
        Ok(())
    }

    fn Clone(&self) -> Result<IStream> {
        Ok(FileStream {
            file: Arc::clone(&self.file),
            length: self.length,
            hidden: Arc::clone(&self.hidden),
            offset: Mutex::new(*self.offset.lock().map_err(|_| STG_E_SEEKERROR)?),
        }
        .into())
    }

    fn SetSize(&self, _: u64) -> Result<()> {
        Err(STG_E_ACCESSDENIED.into())
    }
    fn CopyTo(&self, _: Ref<IStream>, _: u64, _: *mut u64, _: *mut u64) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn Commit(&self, _: &STGC) -> Result<()> {
        Err(STG_E_ACCESSDENIED.into())
    }
    fn Revert(&self) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn LockRegion(&self, _: u64, _: u64, _: &LOCKTYPE) -> Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }
    fn UnlockRegion(&self, _: u64, _: u64, _: u32) -> Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }
}
