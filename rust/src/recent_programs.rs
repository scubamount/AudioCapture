use crate::{prefs, sources};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::mem::{MaybeUninit, size_of};
use std::os::windows::{ffi::OsStringExt, fs::OpenOptionsExt, io::AsRawHandle};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_BASIC_INFO, FILE_ID_INFO, FILE_INFO_BY_HANDLE_CLASS, FILE_NAME_NORMALIZED,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FileBasicInfo, FileIdInfo, GetDriveTypeW,
    GetFileInformationByHandleEx, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
};

const HISTORY_VERSION: u32 = 1;
const MAX_HISTORY: usize = 12;
const MAX_HISTORY_BYTES: u64 = 128 * 1024;
const MAX_PROBES: usize = 256;
const MAX_HASHES: usize = 64;
const HASH_BYTES_PER_SCAN: u64 = 512 * 1024 * 1024;
const MAX_PATH_UNITS: usize = 32_768;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecentProgram {
    pub name: String,
    pub image: PathBuf,
    pub mic: Option<String>,
    sha256: [u8; 32],
    #[serde(default)]
    tree_root: bool,
}

#[derive(Default, Clone, Debug, Deserialize, Serialize)]
pub struct History {
    #[serde(default)]
    automatic: bool,
    #[serde(default)]
    last_single: bool,
    entries: Vec<RecentProgram>,
}

#[derive(Deserialize, Serialize)]
struct StoredHistory {
    version: u32,
    history: History,
}

impl History {
    pub fn load() -> Result<Self, String> {
        Self::read(&prefs::recent_path()?)
    }

    fn read(path: &Path) -> Result<Self, String> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("Cannot open recent programs: {error}")),
        };
        let mut bytes = Vec::new();
        file.take(MAX_HISTORY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("Cannot read recent programs: {error}"))?;
        if bytes.len() as u64 > MAX_HISTORY_BYTES {
            return Err("Recent programs file is too large".into());
        }
        let result: StoredHistory = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Cannot read recent programs: {error}"))?;
        if result.version != HISTORY_VERSION || result.history.entries.len() > MAX_HISTORY {
            return Err("Unsupported recent programs settings".into());
        }
        let mut history = result.history;
        if history.entries.is_empty() {
            history.last_single = false;
        }
        Ok(history)
    }

    pub fn save(&self) -> Result<(), String> {
        self.write(&prefs::recent_path()?)
    }

    fn write(&self, path: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec(&StoredHistory {
            version: HISTORY_VERSION,
            history: self.clone(),
        })
        .map_err(|error| format!("Cannot serialize recent programs: {error}"))?;
        if bytes.len() as u64 > MAX_HISTORY_BYTES {
            return Err("Recent programs file is too large".into());
        }
        prefs::write_atomic(path, &bytes)
    }

    pub fn entries(&self) -> &[RecentProgram] {
        &self.entries
    }

    pub fn auto_select(&self) -> bool {
        self.automatic
    }

    pub fn set_auto_select(&mut self, enabled: bool) {
        self.automatic = enabled;
    }

    pub fn last_single(&self) -> Option<usize> {
        (self.last_single && !self.entries.is_empty()).then_some(0)
    }

    pub fn automatic_candidate(
        &self,
        unresolved_names: &[String],
        remembered_path: Option<&Path>,
    ) -> Option<&RecentProgram> {
        if !self.automatic {
            return None;
        }
        let program = self.entries.get(self.last_single()?)?;
        if !program.tree_root {
            return None;
        }
        (unresolved_names.is_empty()
            || (unresolved_names == [program.name.as_str()]
                && remembered_path == Some(program.image.as_path())))
        .then_some(program)
    }

    pub fn forget(&mut self, index: usize) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        self.entries.remove(index);
        if index == 0 {
            self.last_single = false;
        }
        true
    }

    pub fn clear(&mut self) {
        self.last_single = false;
        self.entries.clear();
    }

    /// Store identities only for the exact process instances that started.
    pub fn remember(
        &mut self,
        snapshot: &Snapshot,
        selected: &[(u32, u64)],
        mic: Option<&str>,
        system: bool,
    ) {
        let mut verified: Vec<_> = selected
            .iter()
            .filter_map(|&(pid, expected)| {
                let candidate = snapshot.images.get(&pid)?;
                if candidate.created != expected {
                    return None;
                }
                let name = snapshot.names.get(&pid)?.clone();
                let sha256 = candidate.sha256?;
                Some(RecentProgram {
                    tree_root: !snapshot.has_same_name_ancestor(pid, &name),
                    name,
                    image: candidate.path.clone(),
                    mic: mic.map(str::to_owned),
                    sha256,
                })
            })
            .collect();
        self.last_single =
            !system && selected.len() == 1 && verified.len() == 1 && verified[0].tree_root;
        verified.sort_by(|a, b| a.image.cmp(&b.image));
        verified.dedup_by(|a, b| a.image == b.image);
        for program in verified.into_iter().rev() {
            self.entries.retain(|entry| entry.image != program.image);
            self.entries.insert(0, program);
        }
        self.entries.truncate(MAX_HISTORY);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    Unique { pid: u32, created: u64 },
    Absent,
    Review,
}

struct ProcessImage {
    created: u64,
    path: PathBuf,
    sha256: Option<[u8; 32]>,
}

pub struct Snapshot {
    pub processes: Vec<sources::Process>,
    pub warning: Option<String>,
    pub inputs: Vec<(String, String)>,
    pub outputs: Vec<(String, String)>,
    pub device_error: Option<String>,
    names: HashMap<u32, String>,
    images: HashMap<u32, ProcessImage>,
    parents: HashMap<u32, u32>,
}

impl Snapshot {
    pub fn image(&self, pid: u32) -> Option<&Path> {
        self.images.get(&pid).map(|image| image.path.as_path())
    }

    pub fn created(&self, pid: u32) -> Option<u64> {
        self.images.get(&pid).map(|image| image.created)
    }

    pub fn resolve(&self, recent: &RecentProgram) -> Match {
        if !recent.tree_root {
            // A previously selected helper only covered its own subtree. Path
            // and fingerprint cannot identify that helper in a new instance.
            return Match::Review;
        }
        let mut matching = Vec::new();
        let mut uncertain = false;
        let mut has_named_candidate = false;
        for (&pid, name) in &self.names {
            if !name.eq_ignore_ascii_case(&recent.name) {
                continue;
            }
            has_named_candidate = true;
            let Some(candidate) = self.images.get(&pid) else {
                uncertain = true;
                continue;
            };
            if candidate.path == recent.image {
                if candidate.sha256 == Some(recent.sha256) {
                    matching.push(pid);
                } else {
                    uncertain = true;
                }
            } else if candidate.sha256.is_none() {
                // An unverified path spelling might still be another alias.
                uncertain = true;
            }
        }
        if uncertain {
            return Match::Review;
        }
        match matching.len() {
            0 if has_named_candidate => Match::Review,
            0 => Match::Absent,
            1 => Match::Unique {
                pid: matching[0],
                created: self.images[&matching[0]].created,
            },
            _ => {
                // Same executable can host helper processes. Keep a single
                // verified root only if the paths between it and its matching
                // children consist of verified instances in creation order.
                let roots: Vec<_> = matching
                    .iter()
                    .copied()
                    .filter(|pid| {
                        let Some(parent) = self.parents.get(pid) else {
                            return true;
                        };
                        !matching.contains(parent)
                            || self.images[parent].created >= self.images[pid].created
                    })
                    .collect();
                match roots.as_slice() {
                    [pid] => Match::Unique {
                        pid: *pid,
                        created: self.images[pid].created,
                    },
                    _ => Match::Review,
                }
            }
        }
    }

    fn has_same_name_ancestor(&self, pid: u32, name: &str) -> bool {
        let mut current = pid;
        for _ in 0..64 {
            let Some(&parent) = self.parents.get(&current) else {
                return false;
            };
            if parent == 0 || parent == current {
                return false;
            }
            if self
                .names
                .get(&parent)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name))
            {
                return true;
            }
            current = parent;
        }
        // A cycle or truncated ancestry cannot establish a verified root.
        true
    }
}

#[derive(Default)]
pub struct Discovery {
    pending: Option<Receiver<(FingerprintCache, Result<Snapshot, String>)>>,
    failed: Option<String>,
    fingerprints: FingerprintCache,
}

impl Discovery {
    /// Admit at most one scan. The caller can request another after polling.
    pub fn refresh(&mut self, history: &History, selected: &[(u32, u64)]) -> bool {
        if self.is_running() {
            return false;
        }
        let entries = history.entries.clone();
        let selected: HashMap<_, _> = selected.iter().copied().take(MAX_PROBES).collect();
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut cache = std::mem::take(&mut self.fingerprints);
        match thread::Builder::new()
            .name("audio-capture-source-discovery".into())
            .spawn(move || {
                let result = scan(&entries, &selected, &mut cache);
                let _ = sender.send((cache, result));
            }) {
            Ok(_) => self.pending = Some(receiver),
            Err(error) => self.failed = Some(format!("Cannot start source discovery: {error}")),
        }
        true
    }

    pub fn poll(&mut self) -> Option<Result<Snapshot, String>> {
        if let Some(error) = self.failed.take() {
            return Some(Err(error));
        }
        let received = self.pending.as_ref()?.try_recv();
        match received {
            Ok((cache, snapshot)) => {
                self.pending = None;
                self.fingerprints = cache;
                Some(snapshot)
            }
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                Some(Err("Source discovery ended without a result".into()))
            }
            Err(TryRecvError::Empty) => None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.pending.is_some() || self.failed.is_some()
    }
}

fn scan(
    entries: &[RecentProgram],
    selected: &HashMap<u32, u64>,
    cache: &mut FingerprintCache,
) -> Result<Snapshot, String> {
    let snapshot = sources::process_snapshot()?;
    let (inputs, outputs, device_error) = match sources::devices() {
        Ok((inputs, outputs)) => (inputs, outputs, None),
        Err(error) => (Vec::new(), Vec::new(), Some(error)),
    };
    let mut names = HashMap::new();
    let mut name_counts = HashMap::<String, usize>::new();
    for process in &snapshot.processes {
        names.insert(process.pid, process.name.clone());
        *name_counts
            .entry(process.name.to_ascii_lowercase())
            .or_default() += 1;
    }

    let priority = |process: &sources::Process| {
        if selected.contains_key(&process.pid) {
            0
        } else if entries
            .iter()
            .any(|entry| process.name.eq_ignore_ascii_case(&entry.name))
        {
            1
        } else if name_counts
            .get(&process.name.to_ascii_lowercase())
            .copied()
            .unwrap_or(0)
            > 1
        {
            2
        } else {
            3
        }
    };
    let mut candidates: Vec<_> = snapshot
        .processes
        .iter()
        .filter_map(|process| {
            let rank = priority(process);
            (rank < 3).then_some((rank, process))
        })
        .collect();
    candidates.sort_by_key(|&(rank, process)| (rank, process.pid));
    let mut images = HashMap::new();
    let mut budget = HASH_BYTES_PER_SCAN;
    for (rank, process) in candidates.into_iter().take(MAX_PROBES) {
        let Ok((created, path)) = sources::process_image(process.pid) else {
            continue;
        };
        if created > snapshot.taken_at || (rank == 0 && selected[&process.pid] != created) {
            continue;
        }
        let mut image = ProcessImage {
            created,
            path,
            sha256: None,
        };
        if rank < 2
            && let Ok((path, fingerprint)) = cache.verify(&image.path, created, &mut budget)
        {
            image.path = path;
            image.sha256 = Some(fingerprint);
        }
        images.insert(process.pid, image);
    }
    Ok(Snapshot {
        processes: snapshot.processes,
        warning: snapshot.warning,
        inputs,
        outputs,
        device_error,
        names,
        images,
        parents: snapshot.parents,
    })
}

#[derive(Default)]
struct FingerprintCache(HashMap<PathBuf, (FileStamp, [u8; 32])>);

impl FingerprintCache {
    fn verify(
        &mut self,
        path: &Path,
        process_created: u64,
        budget: &mut u64,
    ) -> Result<(PathBuf, [u8; 32]), String> {
        if !local_disk(path) {
            return Err("The program image is not on a local drive".into());
        }
        // Excluding FILE_SHARE_WRITE keeps in-place changes out of the hash.
        // FILE_SHARE_DELETE allows updates to replace the path with a new file.
        let mut file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
            .open(path)
            .map_err(|error| format!("Cannot open program image: {error}"))?;
        let canonical = final_path(&file)?;
        let stamp = FileStamp::read(&file)?;
        if stamp.change >= 0 && stamp.change as u64 > process_created {
            return Err("The program image changed after its process started".into());
        }
        if let Some((previous, hash)) = self.0.get(&canonical)
            && *previous == stamp
        {
            return Ok((canonical, *hash));
        }
        if stamp.size > *budget {
            return Err("Program image exceeds the discovery hash budget".into());
        }
        *budget -= stamp.size;
        let mut input = file.by_ref().take(stamp.size);
        let sha256 = sources::sha256(&mut input)?;
        if input.limit() != 0 || FileStamp::read(&file)? != stamp {
            return Err("The program image changed during verification".into());
        }
        if self.0.len() >= MAX_HASHES {
            self.0.clear();
        }
        self.0.insert(canonical.clone(), (stamp, sha256));
        Ok((canonical, sha256))
    }
}

fn local_disk(path: &Path) -> bool {
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return false;
    };
    let (Prefix::Disk(letter) | Prefix::VerbatimDisk(letter)) = prefix.kind() else {
        return false;
    };
    let root = [u16::from(letter), b':' as u16, b'\\' as u16, 0];
    // DRIVE_REMOVABLE, DRIVE_FIXED, DRIVE_CDROM and DRIVE_RAMDISK are local.
    matches!(unsafe { GetDriveTypeW(root.as_ptr()) }, 2 | 3 | 5 | 6)
}

fn final_path(file: &File) -> Result<PathBuf, String> {
    let mut path = vec![0u16; 512];
    loop {
        let count = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                path.as_mut_ptr(),
                path.len() as u32,
                VOLUME_NAME_DOS | FILE_NAME_NORMALIZED,
            )
        } as usize;
        if count == 0 || count >= MAX_PATH_UNITS {
            return Err(format!(
                "Cannot resolve program image: {}",
                std::io::Error::last_os_error()
            ));
        }
        if count < path.len() {
            return Ok(std::ffi::OsString::from_wide(&path[..count]).into());
        }
        path.resize(count + 1, 0);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileStamp {
    volume: u64,
    id: [u8; 16],
    change: i64,
    write: i64,
    size: u64,
}

impl FileStamp {
    fn read(file: &File) -> Result<Self, String> {
        let basic: FILE_BASIC_INFO = read_file_info(file, FileBasicInfo)?;
        let id: FILE_ID_INFO = read_file_info(file, FileIdInfo)?;
        let meta = file
            .metadata()
            .map_err(|error| format!("Cannot inspect program image: {error}"))?;
        if !meta.is_file() {
            return Err("Program image is not a regular file".into());
        }
        Ok(Self {
            volume: id.VolumeSerialNumber,
            id: id.FileId.Identifier,
            change: basic.ChangeTime,
            write: basic.LastWriteTime,
            size: meta.len(),
        })
    }
}

fn read_file_info<T>(file: &File, class: FILE_INFO_BY_HANDLE_CLASS) -> Result<T, String> {
    let mut out = MaybeUninit::<T>::uninit();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            class,
            out.as_mut_ptr().cast(),
            size_of::<T>() as u32,
        )
    } == 0
    {
        return Err(format!(
            "Cannot inspect program image: {}",
            std::io::Error::last_os_error()
        ));
    }
    // The file information class defines T and Windows populated that buffer.
    Ok(unsafe { out.assume_init() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn remembered() -> RecentProgram {
        RecentProgram {
            name: "Program.exe".into(),
            image: r"\\?\C:\Apps\Program.exe".into(),
            mic: Some("mic-identity".into()),
            sha256: [19; 32],
            tree_root: true,
        }
    }

    fn snapshot(program: &RecentProgram, rows: &[(u32, u64, u32)]) -> Snapshot {
        Snapshot {
            processes: Vec::new(),
            warning: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            device_error: None,
            names: rows
                .iter()
                .map(|&(pid, _, _)| (pid, program.name.clone()))
                .collect(),
            images: rows
                .iter()
                .map(|&(pid, created, _)| {
                    (
                        pid,
                        ProcessImage {
                            created,
                            path: program.image.clone(),
                            sha256: Some(program.sha256),
                        },
                    )
                })
                .collect(),
            parents: rows.iter().map(|&(pid, _, parent)| (pid, parent)).collect(),
        }
    }

    #[test]
    fn known_image_can_restart_and_same_tree_helpers_do_not_create_ambiguity() {
        let old = remembered();
        let current = snapshot(&old, &[(22, 100, 0), (23, 101, 22), (24, 102, 23)]);
        assert_eq!(
            current.resolve(&old),
            Match::Unique {
                pid: 22,
                created: 100
            }
        );
    }

    #[test]
    fn two_roots_changed_images_or_unverified_paths_require_review() {
        let old = remembered();
        let mut current = snapshot(&old, &[(44, 100, 0), (55, 104, 0)]);
        assert_eq!(current.resolve(&old), Match::Review);
        current.images.get_mut(&55).unwrap().path = r"\\?\C:\Other\Program.exe".into();
        assert_eq!(
            current.resolve(&old),
            Match::Unique {
                pid: 44,
                created: 100
            }
        );
        current.images.get_mut(&44).unwrap().sha256 = Some([3; 32]);
        assert_eq!(current.resolve(&old), Match::Review);
        current.images.remove(&44);
        assert_eq!(current.resolve(&old), Match::Review);
        current.names.clear();
        assert_eq!(current.resolve(&old), Match::Absent);
    }

    #[test]
    fn reused_parent_pid_cannot_hide_a_second_matching_root() {
        let old = remembered();
        let current = snapshot(&old, &[(44, 110, 0), (55, 104, 44)]);
        assert_eq!(current.resolve(&old), Match::Review);
    }

    #[test]
    fn captured_helper_cannot_restore_the_broader_program_tree() {
        let old = remembered();
        let current = snapshot(&old, &[(20, 100, 0), (21, 110, 20)]);
        let mut history = History::default();
        history.remember(&current, &[(21, 110)], None, false);
        history.set_auto_select(true);
        assert!(history.automatic_candidate(&[], None).is_none());
        assert_eq!(current.resolve(&history.entries()[0]), Match::Review);
        history.remember(&current, &[(20, 100)], None, false);
        assert_eq!(
            current.resolve(history.automatic_candidate(&[], None).unwrap()),
            Match::Unique {
                pid: 20,
                created: 100
            }
        );
    }

    #[test]
    fn a_reused_pid_is_not_remembered_and_forget_disables_old_preselection() {
        let program = remembered();
        let current = snapshot(&program, &[(90, 13, 0)]);
        let mut history = History::default();
        assert!(!history.auto_select());
        history.remember(&current, &[(90, 9)], Some("invalid"), false);
        assert!(history.entries().is_empty());

        history.remember(&current, &[(90, 13)], Some("real-mic"), false);
        assert_eq!(history.last_single(), Some(0));
        assert_eq!(history.entries()[0].mic.as_deref(), Some("real-mic"));
        history.remember(&current, &[(90, 13)], Some("real-mic"), true);
        assert_eq!(history.last_single(), None);
        history.remember(&current, &[(90, 13)], Some("real-mic"), false);
        assert!(history.forget(0));
        assert!(history.entries().is_empty());
        assert_eq!(history.last_single(), None);
    }

    #[test]
    fn automatic_selection_respects_pending_legacy_scope_and_opt_in() {
        let program = remembered();
        let current = snapshot(&program, &[(90, 13, 0)]);
        let mut history = History::default();
        history.remember(&current, &[(90, 13)], program.mic.as_deref(), false);
        assert!(history.automatic_candidate(&[], None).is_none());
        history.set_auto_select(true);
        assert_eq!(history.automatic_candidate(&[], None), Some(&program));
        let legacy = [program.name.clone()];
        assert!(history.automatic_candidate(&legacy, None).is_none());
        assert_eq!(
            history.automatic_candidate(&legacy, Some(&program.image)),
            Some(&program)
        );
        assert!(
            history
                .automatic_candidate(&["different.exe".into()], Some(&program.image))
                .is_none()
        );
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
            let suffix = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "audio-capture-history-{}-{suffix}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn history_round_trip_is_bounded_and_replaces_previous_settings() {
        let scratch = Scratch::new();
        let path = scratch.0.join("history.json");
        let mut history = History::read(&path).unwrap();
        for index in 0..MAX_HISTORY + 3 {
            let mut program = remembered();
            program.image = format!(r"\\?\C:\Example{index}\Program.exe").into();
            let current = snapshot(&program, &[(100, 10, 0)]);
            history.remember(&current, &[(100, 10)], Some("mic"), false);
        }
        assert_eq!(history.entries().len(), MAX_HISTORY);
        history.set_auto_select(true);
        history.write(&path).unwrap();
        let mut stored = History::read(&path).unwrap();
        assert!(stored.auto_select());
        assert_eq!(stored.last_single(), Some(0));
        assert_eq!(stored.entries(), history.entries());
        stored.clear();
        stored.write(&path).unwrap();
        assert!(History::read(&path).unwrap().entries().is_empty());

        let mut file = File::create(&path).unwrap();
        file.write_all(&vec![b'x'; MAX_HISTORY_BYTES as usize + 1])
            .unwrap();
        assert!(History::read(&path).unwrap_err().contains("too large"));
    }

    #[test]
    fn fingerprint_cache_invalidates_replaced_file_and_respects_hash_budget() {
        let scratch = Scratch::new();
        let path = scratch.0.join("program.exe");
        let replacement = scratch.0.join("replacement.exe");
        std::fs::write(&path, b"image-a").unwrap();
        std::fs::write(&replacement, b"image-b").unwrap();
        let mut cache = FingerprintCache::default();
        assert!(cache.verify(&path, u64::MAX, &mut 0).is_err());
        let (_, old) = cache.verify(&path, u64::MAX, &mut 7).unwrap();
        let (_, reused) = cache.verify(&path, u64::MAX, &mut 0).unwrap();
        assert_eq!(old, reused);

        std::fs::rename(&replacement, &path).unwrap();
        assert!(cache.verify(&path, 0, &mut 7).is_err());
        let (_, new) = cache.verify(&path, u64::MAX, &mut 7).unwrap();
        assert_ne!(old, new);
    }

    #[test]
    fn discovery_coalesces_requests_and_verifies_an_actual_process() {
        let pid = std::process::id();
        let created = sources::process_identity(pid).unwrap();
        let mut discovery = Discovery::default();
        assert!(discovery.refresh(&History::default(), &[(pid, created)]));
        assert!(!discovery.refresh(&History::default(), &[]));
        let mut snapshot = None;
        for _ in 0..100 {
            if let Some(completed) = discovery.poll() {
                snapshot = Some(completed.unwrap());
                break;
            }
            thread::sleep(Duration::from_millis(40));
        }
        let snapshot = snapshot.expect("Discovery must complete");
        assert_eq!(snapshot.created(pid), Some(created));
        let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
        assert_eq!(snapshot.image(pid), Some(executable.as_path()));
        let mut history = History::default();
        history.remember(&snapshot, &[(pid, created)], None, false);
        let [entry] = history.entries() else {
            panic!("Expected one verified executable in history");
        };
        let expected = sources::sha256(File::open(&executable).unwrap()).unwrap();
        assert_eq!(entry.image, executable);
        assert_eq!(entry.sha256, expected);
        // Other tests can spawn the same executable. The deterministic
        // snapshots above test whether a set of candidates is unambiguous.
        assert!(!discovery.is_running());
    }
}
