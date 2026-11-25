//! Windows identities and durable state for local transcription.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::ffi::{OsString, c_void};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::mem::{MaybeUninit, size_of};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, LocalFree,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
    EqualSid, GetAce, GetKernelObjectSecurity, GetLengthSid, GetSecurityDescriptorControl,
    GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, GetTokenInformation, IsWellKnownSid,
    OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES,
    SECURITY_MAX_SID_SIZE, TOKEN_QUERY, TOKEN_USER, TokenUser, WinCreatorOwnerRightsSid,
    WinLocalSystemSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, DELETE, FILE_ALL_ACCESS,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_BASIC_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileBasicInfo, GetDriveTypeW,
    GetFileInformationByHandle, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, VOLUME_NAME_DOS,
};

use windows_sys::Win32::Storage::FileSystem::{
    FILE_RENAME_INFO, FILE_RENAME_INFO_0, FileRenameInfoEx, SetFileInformationByHandle,
};

use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const MAX_STATE_BYTES: u64 = 1024 * 1024;
const MAX_PATH_UNITS: usize = 32_767;
const LOCK_WAIT: Duration = Duration::from_secs(5);

#[link(name = "advapi32")]
unsafe extern "system" {
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        sddl: *const u16,
        revision: u32,
        descriptor: *mut *mut c_void,
        length: *mut u32,
    ) -> i32;
    fn ConvertSidToStringSidW(sid: PSID, description: *mut *mut u16) -> i32;
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct Identity {
    volume: u32,
    file_index: u64,
    bytes: u64,
    changed_at: i64,
    last_write_at: i64,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct DirIdentity {
    volume: u32,
    file_index: u64,
}

pub(super) fn dir_identity(directory: &File) -> Result<DirIdentity, String> {
    let info = file_info(directory).map_err(|e| format!("Cannot identify local directory: {e}"))?;
    if is_reparse(&info) || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err("Cannot identify a linked or non-directory path".into());
    }
    Ok(DirIdentity {
        volume: info.dwVolumeSerialNumber,
        file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

pub(super) fn root() -> Result<PathBuf, String> {
    let path = if let Some(path) = std::env::var_os("AUDIO_CAPTURE_STATE_DIR") {
        PathBuf::from(path)
    } else {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or("LOCALAPPDATA is unavailable")?
            .join("AudioCaptureRust")
            .join("native-control-v1")
    };
    let (root, path, _) = walk(&path, Kind::Directory, true)?;
    check_private_directory(&root)?;
    for name in ["grants", "plans", "jobs", "receipts"] {
        let child = walk_directory(&path.join(name), true)?;
        check_private_directory(&child)?;
    }
    Ok(path)
}

pub(super) fn id(prefix: &str) -> Result<String, String> {
    crate::attribution::model::new_id(prefix)
}

pub(super) fn valid_id(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix)
        .and_then(|suffix| suffix.strip_prefix('_'))
        .is_some_and(|hex| {
            hex.len() == 32
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// Compare resolved paths component by component. A Windows directory can
/// enable case-sensitive names, so folding case might authorize a sibling.
pub(super) fn is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

/// Normalize a drive-qualified path before any file or volume access.
pub(super) fn lexical_path(path: &Path) -> Option<PathBuf> {
    let mut parts = path.components();
    let Component::Prefix(prefix) = parts.next()? else {
        return None;
    };
    let (Prefix::Disk(drive) | Prefix::VerbatimDisk(drive)) = prefix.kind() else {
        return None;
    };
    if parts.next() != Some(Component::RootDir) {
        return None;
    }
    let mut normalized =
        PathBuf::from(format!(r"\\?\{}:\", char::from(drive.to_ascii_uppercase())));
    for part in parts {
        let Component::Normal(name) = part else {
            return None;
        };
        let units = name.encode_wide();
        if units
            .clone()
            .any(|unit| matches!(unit, 0..=31 | 34 | 42 | 47 | 58 | 60 | 62 | 63 | 92 | 124))
            || matches!(units.last(), None | Some(32 | 46))
        {
            return None;
        }
        normalized.push(name);
    }
    Some(normalized)
}

pub(super) fn local_path(path: &Path) -> Result<PathBuf, String> {
    walk(path, Kind::Either, false).map(|(_, resolved, _)| resolved)
}

pub(super) fn hold_dir(path: &Path) -> Result<(File, PathBuf), String> {
    walk(path, Kind::Directory, false).map(|(file, resolved, _)| (file, resolved))
}

pub(super) fn open_input(path: &Path) -> Result<(File, PathBuf, Identity), String> {
    let (file, path, info) = walk(path, Kind::File, false)?;
    let mut times = MaybeUninit::<FILE_BASIC_INFO>::uninit();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            times.as_mut_ptr().cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    } == 0
    {
        return Err(format!(
            "Cannot read the input file identity: {}",
            io::Error::last_os_error()
        ));
    }
    let times = unsafe { times.assume_init() };
    let join = |hi, lo| (u64::from(hi) << 32) | u64::from(lo);
    let identity = Identity {
        volume: info.dwVolumeSerialNumber,
        file_index: join(info.nFileIndexHigh, info.nFileIndexLow),
        bytes: join(info.nFileSizeHigh, info.nFileSizeLow),
        changed_at: times.ChangeTime,
        last_write_at: times.LastWriteTime,
    };
    Ok((file, path, identity))
}

pub(super) fn read<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .and_then(check_state_file)
        .map_err(|e| format!("Cannot open local state record: {e}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read local state record: {e}"))?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err("Local state record exceeds 1 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid local state record".into())
}

pub(super) fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|_| "Cannot encode local state record")?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err("Local state record exceeds 1 MiB".into());
    }
    let pending = path.with_file_name(id("pending")?);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)
        .map_err(|e| format!("Cannot stage local state record: {e}"))?;
    let staged = file.write_all(&bytes).and_then(|()| file.sync_all());
    drop(file);
    let result = staged.and_then(|()| publish(&pending, path, true));
    if let Err(error) = result {
        let _ = fs::remove_file(&pending);
        return Err(format!("Cannot publish local state record: {error}"));
    }
    Ok(())
}

pub(super) fn lock(path: &Path) -> Result<File, String> {
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        if let Some(file) = try_lock(path)? {
            return Ok(file);
        }
        if Instant::now() >= deadline {
            return Err("Local transcription state is busy. Try again shortly.".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn try_lock(path: &Path) -> Result<Option<File>, String> {
    match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .and_then(check_state_file)
    {
        Ok(file) => Ok(Some(file)),
        Err(error)
            if error.raw_os_error().is_some_and(|code| {
                matches!(code as u32, ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
            }) =>
        {
            Ok(None)
        }
        Err(error) => Err(format!("Cannot lock local transcription state: {error}")),
    }
}

pub(super) fn publish(from: &Path, to: &Path, replace: bool) -> io::Result<()> {
    let mut flags = MOVEFILE_WRITE_THROUGH;
    if replace {
        flags |= MOVEFILE_REPLACE_EXISTING;
    }
    let source = wide(from);
    let destination = wide(to);
    for delay in [5, 15, 35, 75, 150, 0] {
        if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), flags) } != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if !error.raw_os_error().is_some_and(|code| {
            matches!(code as u32, ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)
        }) {
            return Err(error);
        }
        // Classic Windows rename rejects a destination with an open handle.
        // POSIX replacement keeps existing readers on their original snapshot.
        if replace && replace_open_record(from, to).is_ok() {
            return Ok(());
        }
        if delay == 0 {
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(delay));
    }
    unreachable!("The final publish attempt returns")
}

fn replace_open_record(from: &Path, to: &Path) -> io::Result<()> {
    const REPLACE_EXISTING: u32 = 0x1;
    const POSIX_SEMANTICS: u32 = 0x2;
    let file = OpenOptions::new()
        .access_mode(DELETE)
        .share_mode(FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(from)
        .and_then(check_state_file)?;
    let name: Vec<_> = to.as_os_str().encode_wide().collect();
    let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let name_bytes = name.len() * size_of::<u16>();
    let size = (offset + name_bytes).max(size_of::<FILE_RENAME_INFO>());
    let mut buffer = vec![0_usize; size.div_ceil(size_of::<usize>())];
    let header = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    let value = FILE_RENAME_INFO {
        Anonymous: FILE_RENAME_INFO_0 {
            Flags: REPLACE_EXISTING | POSIX_SEMANTICS,
        },
        RootDirectory: std::ptr::null_mut(),
        FileNameLength: name_bytes as u32,
        FileName: [0],
    };
    // The aligned buffer holds the C header and its variable-length filename.
    let renamed = unsafe {
        header.write(value);
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            buffer.as_mut_ptr().cast::<u8>().add(offset).cast(),
            name.len(),
        );
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfoEx,
            header.cast(),
            size as u32,
        )
    };
    if renamed == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

struct UserIdentity {
    sid: [u32; SECURITY_MAX_SID_SIZE as usize / size_of::<u32>()],
    private_sddl: Vec<u16>,
}

impl UserIdentity {
    fn load() -> io::Result<Self> {
        let mut token = std::ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut user = [0_usize; 32];
        let mut size = 0;
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                user.as_mut_ptr().cast(),
                size_of_val(&user) as u32,
                &mut size,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let source = unsafe { user.as_ptr().cast::<TOKEN_USER>().read().User.Sid };
        let mut sid = [0; SECURITY_MAX_SID_SIZE as usize / size_of::<u32>()];
        let size = unsafe { GetLengthSid(source) } as usize;
        if size == 0 || size > size_of_val(&sid) {
            return Err(io::Error::other("Invalid Windows account identity"));
        }
        unsafe {
            std::ptr::copy_nonoverlapping(source.cast::<u8>(), sid.as_mut_ptr().cast(), size);
        }
        let mut owner = std::ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(source, &mut owner) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // A Windows SID string contains at most 15 subauthorities.
        let owner_text = unsafe {
            let length = (0..184).find(|offset| owner.add(*offset).read() == 0);
            let result = length
                .map(|length| String::from_utf16_lossy(std::slice::from_raw_parts(owner, length)));
            LocalFree(owner.cast());
            result
        }
        .ok_or_else(|| io::Error::other("Invalid Windows account SID"))?;
        let private_sddl = format!("O:{owner_text}D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        Ok(Self { sid, private_sddl })
    }

    fn sid(&self) -> PSID {
        self.sid.as_ptr().cast_mut().cast()
    }
}

fn user_identity() -> io::Result<&'static UserIdentity> {
    static USER: OnceLock<Result<UserIdentity, String>> = OnceLock::new();
    USER.get_or_init(|| UserIdentity::load().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|error| io::Error::other(error.clone()))
}

fn check_private_directory(directory: &File) -> Result<(), String> {
    let user =
        user_identity().map_err(|e| format!("Cannot check Windows account identity: {e}"))?;
    let mut buffer = [0_usize; 64];
    let mut bytes = 0;
    let descriptor = buffer.as_mut_ptr().cast();
    let checked = unsafe {
        // Query the held handle to avoid opening an object by path twice.
        GetKernelObjectSecurity(
            directory.as_raw_handle(),
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            descriptor,
            size_of_val(&buffer) as u32,
            &mut bytes,
        ) != 0
            && check_private_acl(descriptor, user)
    };
    if !checked {
        return Err(
            "Local state needs a private directory owned by your Windows account. Select a new state directory."
                .into(),
        );
    }
    Ok(())
}

/// Accept only inheritable account, owner rights, and system permissions.
unsafe fn check_private_acl(descriptor: *mut c_void, user: &UserIdentity) -> bool {
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    let mut owner = std::ptr::null_mut();
    let mut defaulted = 0;
    let mut present = 0;
    let mut acl: *mut ACL = std::ptr::null_mut();
    let mut control = 0;
    let mut revision = 0;
    unsafe {
        if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0
            || control & SE_DACL_PROTECTED == 0
            || GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) == 0
            || owner.is_null()
            || EqualSid(owner, user.sid()) == 0
            || GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) == 0
            || present == 0
            || acl.is_null()
        {
            return false;
        }
        let mut owner_allowed = false;
        for index in 0..u32::from((*acl).AceCount) {
            let mut entry = std::ptr::null_mut();
            if GetAce(acl, index, &mut entry) == 0 || entry.is_null() {
                return false;
            }
            let header = &*entry.cast::<ACE_HEADER>();
            let inheritable = (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) as u8;
            if header.AceType != ACCESS_ALLOWED_ACE_TYPE
                || header.AceFlags != inheritable
                || usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>()
            {
                return false;
            }
            let ace = &*entry.cast::<ACCESS_ALLOWED_ACE>();
            let sid = std::ptr::addr_of!(ace.SidStart).cast_mut().cast();
            if EqualSid(sid, user.sid()) != 0 || IsWellKnownSid(sid, WinCreatorOwnerRightsSid) != 0
            {
                owner_allowed |= ace.Mask == FILE_ALL_ACCESS;
            } else if IsWellKnownSid(sid, WinLocalSystemSid) == 0 {
                return false;
            }
        }
        owner_allowed
    }
}

fn ensure_private_directory(path: &Path) -> io::Result<()> {
    let sddl = &user_identity()?.private_sddl;
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
        return Err(io::Error::last_os_error());
    }
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let created = unsafe { CreateDirectoryW(wide(path).as_ptr(), &security) };
    let error = (created == 0).then(io::Error::last_os_error);
    unsafe { LocalFree(descriptor) };
    match error {
        None => Ok(()),
        Some(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Some(e) => Err(e),
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Either,
    File,
    Directory,
}

fn local_drive(prefix: Prefix<'_>) -> Option<u8> {
    let (Prefix::Disk(drive) | Prefix::VerbatimDisk(drive)) = prefix else {
        return None;
    };
    let root = [u16::from(drive), b':' as u16, b'\\' as u16, 0];
    // DRIVE_REMOVABLE, DRIVE_FIXED, DRIVE_CDROM, and DRIVE_RAMDISK are local.
    matches!(unsafe { GetDriveTypeW(root.as_ptr()) }, 2 | 3 | 5 | 6).then_some(drive)
}

fn final_path(file: &File) -> Result<PathBuf, String> {
    let mut path = vec![0u16; 512];
    loop {
        let count = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                path.as_mut_ptr(),
                path.len() as u32,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        } as usize;
        if count == 0 {
            return Err(format!(
                "Cannot resolve a local path: {}",
                io::Error::last_os_error()
            ));
        }
        if count >= MAX_PATH_UNITS {
            return Err("Resolved local path exceeds the Windows limit".into());
        }
        if count < path.len() {
            let resolved = PathBuf::from(OsString::from_wide(&path[..count]));
            if let Some(Component::Prefix(prefix)) = resolved.components().next()
                && local_drive(prefix.kind()).is_some()
            {
                return Ok(resolved);
            }
            return Err("Resolved path is outside a local disk".into());
        }
        path.resize(count + 1, 0);
    }
}

fn walk(
    path: &Path,
    kind: Kind,
    create: bool,
) -> Result<(File, PathBuf, BY_HANDLE_FILE_INFORMATION), String> {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Err("Use a fully qualified local Windows path".into());
    };
    let drive = local_drive(prefix.kind()).ok_or("Use a path on a local Windows disk")?;
    if components.next() != Some(Component::RootDir) {
        return Err("Use a fully qualified local Windows path".into());
    }
    let mut current = PathBuf::from(format!(r"\\?\{}:\", char::from(drive)));
    let mut parents =
        vec![open_directory(&current).map_err(|e| format!("Cannot open local disk: {e}"))?];
    let mut components = components.peekable();
    while let Some(Component::Normal(name)) = components.peek().copied() {
        if name.encode_wide().any(|unit| matches!(unit, 0 | 58)) || name == "." || name == ".." {
            return Err("Use a local path without parent traversal or alternate streams".into());
        }
        components.next();
        current.push(name);
        if components.peek().is_none() {
            break;
        }
        parents.push(walk_directory(&current, create)?);
    }
    if components.peek().is_some() {
        return Err("Use a local path without parent traversal".into());
    }
    let file = open_node(&current, kind)
        .or_else(|e| {
            if create && e.kind() == io::ErrorKind::NotFound {
                ensure_private_directory(&current)?;
                open_node(&current, kind)
            } else {
                Err(e)
            }
        })
        .map_err(|e| format!("Cannot open local path: {e}"))?;
    let info = file_info(&file).map_err(|e| format!("Cannot inspect local path: {e}"))?;
    if is_reparse(&info) {
        return Err("Choose a local path without junctions or symbolic links".into());
    }
    let directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    if (matches!(kind, Kind::Directory) && !directory) || (matches!(kind, Kind::File) && directory)
    {
        return Err("The local path has the wrong file type".into());
    }
    let resolved = final_path(&file)?;
    Ok((file, resolved, info))
}

fn walk_directory(path: &Path, create: bool) -> Result<File, String> {
    open_directory(path)
        .or_else(|e| {
            if create && e.kind() == io::ErrorKind::NotFound {
                ensure_private_directory(path)?;
                open_directory(path)
            } else {
                Err(e)
            }
        })
        .map_err(|e| format!("Cannot open local directory: {e}"))
}

fn open_node(path: &Path, kind: Kind) -> io::Result<File> {
    let share = if matches!(kind, Kind::File) {
        FILE_SHARE_READ
    } else {
        FILE_SHARE_READ | FILE_SHARE_WRITE
    };
    OpenOptions::new()
        .read(true)
        .share_mode(share)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

fn open_directory(path: &Path) -> io::Result<File> {
    let file = open_node(path, Kind::Directory)?;
    let info = file_info(&file)?;
    if is_reparse(&info) || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(io::Error::other(
            "Choose a local directory without junctions or symbolic links",
        ));
    }
    Ok(file)
}

fn file_info(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = MaybeUninit::uninit();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { info.assume_init() })
}

fn is_reparse(info: &BY_HANDLE_FILE_INFORMATION) -> bool {
    info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn check_state_file(file: File) -> io::Result<File> {
    let info = file_info(&file)?;
    if is_reparse(&info)
        || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
        || info.nNumberOfLinks != 1
    {
        return Err(io::Error::other(
            "Unexpected linked or non-file state record",
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn lexical_paths_normalize_only_the_drive_and_dos_prefix() {
        let root = lexical_path(Path::new(r"c:\Media")).unwrap();
        assert_eq!(root, PathBuf::from(r"\\?\C:\Media"));
        assert_eq!(lexical_path(Path::new(r"\\?\c:\Media")), Some(root.clone()));
        let inside = lexical_path(Path::new(r"C:/Media/Call.mp4")).unwrap();
        let different_case = lexical_path(Path::new(r"C:\media\Call.mp4")).unwrap();
        let sibling = lexical_path(Path::new(r"C:\Media2\Call.mp4")).unwrap();
        assert!(is_within(&inside, &root));
        assert!(!is_within(&different_case, &root));
        assert!(!is_within(&sibling, &root));
    }

    #[test]
    fn lexical_paths_reject_traversal_streams_and_other_namespaces() {
        for path in [
            "relative.mp4",
            r"C:relative.mp4",
            r"\Media\Call.mp4",
            r"\\server\Media\Call.mp4",
            r"\\?\UNC\server\Media\Call.mp4",
            r"\\.\C:\Media\Call.mp4",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\Call.mp4",
            r"C:\Media\..\Call.mp4",
            r"\\?\C:\Media\..\Call.mp4",
            r"C:\Media\Call.mp4:private",
            r"C:\Media \Call.mp4",
            r"C:\Media.\Call.mp4",
            r"C:\Media\Call?.mp4",
            "C:\\Media\\Call\u{0}.mp4",
        ] {
            assert!(lexical_path(Path::new(path)).is_none(), "{path:?}");
        }
    }

    fn temp() -> PathBuf {
        let path = std::env::temp_dir().join(id("local_batch_test").unwrap());
        ensure_private_directory(&path).unwrap();
        path
    }

    #[test]
    fn private_state_rejects_an_unprotected_existing_directory() {
        let container = temp();
        let ordinary = container.join("ordinary");
        fs::create_dir(&ordinary).unwrap();
        assert!(check_private_directory(&open_directory(&ordinary).unwrap()).is_err());

        let private = container.join("private");
        ensure_private_directory(&private).unwrap();
        let (handle, canonical) = hold_dir(&private).unwrap();
        check_private_directory(&handle).unwrap();
        let first = dir_identity(&handle).unwrap();
        let artifact = canonical.join("test-artifact.txt");
        let staged = canonical.join("staged-artifact.txt");
        fs::write(&staged, b"complete").unwrap();
        publish(&staged, &artifact, false).unwrap();
        assert_eq!(dir_identity(&handle).unwrap(), first);
        assert!(fs::rename(&private, container.join("old")).is_err());
        fs::remove_file(artifact).unwrap();
        drop(handle);
        fs::rename(&private, container.join("old")).unwrap();
        ensure_private_directory(&private).unwrap();
        assert_ne!(
            dir_identity(&open_directory(&private).unwrap()).unwrap(),
            first
        );
        fs::remove_dir(container.join("old")).unwrap();
        fs::remove_dir(ordinary).unwrap();
        fs::remove_dir(private).unwrap();
        fs::remove_dir(container).unwrap();
    }

    #[test]
    fn held_input_denies_mutation_and_detects_replacement() {
        let dir = temp();
        let path = dir.join("A file.mp4");
        fs::write(&path, b"original").unwrap();
        let (file, resolved, first) = open_input(&path).unwrap();
        assert!(is_within(&resolved, &local_path(&dir).unwrap()));
        assert!(fs::write(&path, b"changed").is_err());
        assert!(fs::rename(&path, dir.join("other")).is_err());
        drop(file);
        let old = dir.join("previous");
        fs::rename(&path, &old).unwrap();
        fs::write(&path, b"original").unwrap();
        assert_ne!(open_input(&path).unwrap().2, first);
        assert!(open_input(&path.with_file_name("A file.mp4:other")).is_err());
        assert!(local_path(Path::new(r"\\server\share")).is_err());
        assert!(local_path(Path::new(r"C:relative")).is_err());
        fs::remove_file(old).unwrap();
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn holds_are_exclusive_and_publish_preserves_read_snapshot() {
        let dir = temp();
        let lease = dir.join("job.lock");
        let file = lock(&lease).unwrap();
        assert!(try_lock(&lease).unwrap().is_none());
        drop(file);
        drop(try_lock(&lease).unwrap().unwrap());
        let state = dir.join("state.json");
        write(&state, &json!({"state":"queued"})).unwrap();
        let mut old = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
            .open(&state)
            .unwrap();
        write(&state, &json!({"state":"running"})).unwrap();
        let current: Value = read(&state).unwrap();
        let mut prior = String::new();
        old.read_to_string(&mut prior).unwrap();
        assert_eq!(current["state"], "running");
        assert_eq!(
            serde_json::from_str::<Value>(&prior).unwrap()["state"],
            "queued"
        );
        drop(old);
        fs::remove_file(state).unwrap();
        fs::remove_file(lease).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
