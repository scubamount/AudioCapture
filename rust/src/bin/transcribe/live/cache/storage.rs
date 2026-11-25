use super::Result;
use serde::{Serialize, de::DeserializeOwned};
use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, DELETE, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_DISPOSITION_INFO, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FileDispositionInfo,
    GetFileInformationByHandle, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    SetFileInformationByHandle,
};

const MAX_STATE_BYTES: usize = 1_048_576;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        sddl: *const u16,
        revision: u32,
        descriptor: *mut *mut c_void,
        length: *mut u32,
    ) -> i32;
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn private_directory(path: &Path) -> std::io::Result<()> {
    // OWNER RIGHTS and SYSTEM. OI and CI apply to the TXT, journals, and any
    // child directory. A protected ACL avoids inheriting broad access.
    let sddl: Vec<_> = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let ok = unsafe { CreateDirectoryW(wide(path).as_ptr(), &security) };
    let result = if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    };
    unsafe { LocalFree(descriptor) };
    result
}

fn check_directory(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("Cannot inspect cache directory {}: {e}", path.display()))?;
    if !meta.is_dir() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err("Live cache must use ordinary directories".into());
    }
    Ok(())
}

pub(super) fn local_root() -> Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or("LOCALAPPDATA is unavailable")?;
    if !base.is_absolute() {
        return Err("LOCALAPPDATA must be absolute".into());
    }
    let application = base.join("AudioCaptureRust");
    for path in [&application, &application.join("live-v1")] {
        match private_directory(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(format!("Cannot create private cache directory: {e}")),
        }
        check_directory(path)?;
    }
    Ok(application.join("live-v1"))
}

pub(super) struct OwnedDir {
    pub path: PathBuf,
}

impl OwnedDir {
    pub fn create(root: &Path, id: &str) -> Result<Self> {
        check_directory(root)?;
        let path = root.join(format!("interval-{id}"));
        private_directory(&path).map_err(|e| format!("Cannot create new live interval: {e}"))?;
        Ok(Self { path })
    }
    pub fn existing(root: &Path, id: &str) -> Result<Self> {
        check_directory(root)?;
        let path = root.join(format!("interval-{id}"));
        check_directory(&path)?;
        Ok(Self { path })
    }
}

pub(super) fn create_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Cannot create {}: {e}", path.display()))
}

pub(super) fn open_owned(path: &Path, write: bool) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    check_file(&file, path)?;
    Ok(file)
}

fn check_file(file: &File, path: &Path) -> Result<()> {
    let info = file_info(file)?;
    if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | 0x10) != 0
        || info.nNumberOfLinks != 1
    {
        return Err(format!(
            "Refusing a linked or non-file cache entry: {}",
            path.display()
        ));
    }
    Ok(())
}

fn file_info(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(info)
}

pub(super) fn identity(file: &File) -> Result<(u32, u64)> {
    let info = file_info(file)?;
    Ok((
        info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

pub(super) fn owned_file(path: &Path) -> Result<()> {
    open_owned(path, false).map(drop)
}

pub(super) fn unlink_owned(path: &Path) -> Result<()> {
    unlink_matching(path, None)
}

pub(super) fn unlink_matching(path: &Path, expected: Option<(u32, u64)>) -> Result<()> {
    // Mark the verified handle for deletion. Replacing its path between a
    // metadata check and a separate remove_file call must not delete a new file.
    let file = match OpenOptions::new()
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("Cannot clean {}: {e}", path.display())),
    };
    check_file(&file, path)?;
    if let Some(expected) = expected
        && identity(&file)? != expected
    {
        return Ok(());
    }
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            std::ptr::from_ref(&disposition).cast(),
            std::mem::size_of_val(&disposition) as u32,
        )
    } == 0
    {
        return Err(format!(
            "Cannot clean {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub(super) fn rename(from: &Path, to: &Path, replace: bool) -> std::io::Result<()> {
    let mut flags = MOVEFILE_WRITE_THROUGH;
    if replace {
        flags |= MOVEFILE_REPLACE_EXISTING;
    }
    if unsafe { MoveFileExW(wide(from).as_ptr(), wide(to).as_ptr(), flags) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = open_owned(path, false)?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if size > MAX_STATE_BYTES as u64 {
        return Err("Live interval state exceeds its size limit".into());
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_STATE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err("Live interval state exceeds its size limit".into());
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| format!("Invalid live state {}: {e}", path.display()))
}

pub(super) fn write_json(dir: &Path, name: &str, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err("Live interval state exceeds its size limit".into());
    }
    let temp = dir.join("state.pending");
    unlink_owned(&temp)?;
    let mut file = create_file(&temp)?;
    file.write_all(&bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    let destination = dir.join(name);
    // A viewer or scanner can briefly hold the destination without delete
    // sharing. Keep the old checkpoint valid and retry only the final rename.
    let mut delays = [5, 15, 35, 70, 125].into_iter();
    loop {
        match rename(&temp, &destination, true) {
            Ok(()) => return Ok(()),
            Err(e) if matches!(e.raw_os_error(), Some(5 | 32 | 33)) => {
                if let Some(delay) = delays.next() {
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                } else {
                    return Err(format!("Cannot commit live state: {e}"));
                }
            }
            Err(e) => return Err(format!("Cannot commit live state: {e}")),
        }
    }
}
