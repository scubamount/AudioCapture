//! One elected worker grants slots to recorder-launched jobs. Each grant belongs to
//! a Windows file handle, so exiting a worker releases even an interrupted grant.
use super::{atomic_json, id, read_json, state_root};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Kind {
    Live,
    Batch,
}

#[derive(Serialize, Deserialize)]
struct Request {
    destination: String,
    kind: Kind,
    issued_ns: u128,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Limits {
    total: usize,
    per_destination: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            total: 1,
            per_destination: 1,
        }
    }
}

pub(super) fn exclusive(path: &Path) -> io::Result<Option<File>> {
    match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
    {
        Ok(handle) => Ok(Some(handle)),
        Err(error) if error.raw_os_error() == Some(32) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) struct Admission {
    dir: PathBuf,
    lease: Option<File>,
}

impl Drop for Admission {
    fn drop(&mut self) {
        // Remove the request before releasing the handle. Never traverse a
        // caller-supplied directory or remove an unrelated user's files.
        let _ = fs::remove_file(self.dir.join("request.json"));
        self.lease.take();
        clean(&self.dir);
    }
}

fn clean(dir: &Path) {
    for name in ["request.json", "granted", "lease"] {
        let _ = fs::remove_file(dir.join(name));
    }
    let _ = fs::remove_dir(dir);
}

fn allocate(root: &Path, kind: Kind, destination: &str) -> Result<Admission, String> {
    let dir = root.join(id("request")?);
    fs::create_dir(&dir).map_err(|e| e.to_string())?;
    let lease = exclusive(&dir.join("lease"))
        .map_err(|e| e.to_string())?
        .ok_or("Request lease in use")?;
    let admission = Admission {
        dir,
        lease: Some(lease),
    };
    atomic_json(
        &admission.dir.join("request.json"),
        &Request {
            destination: destination.to_owned(),
            kind,
            issued_ns: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        },
    )?;
    Ok(admission)
}

fn schedule(root: &Path, limits: &Limits) -> Result<(), String> {
    let mut used = 0;
    let mut counts = HashMap::<String, usize>::new();
    let mut pending = Vec::new();
    for entry in fs::read_dir(root).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_name().to_string_lossy().starts_with("request_")
            || !entry.file_type().map_err(|e| e.to_string())?.is_dir()
        {
            continue;
        }
        let dir = entry.path();
        let path = dir.join("request.json");
        if !path.is_file() {
            continue;
        }
        match exclusive(&dir.join("lease")) {
            Ok(Some(stale)) => {
                drop(stale);
                clean(&dir);
                continue;
            }
            Ok(None) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        }
        let request: Request = match read_json(&path) {
            Ok(r) => r,
            Err(_) if !path.exists() => continue,
            Err(e) => return Err(e),
        };
        if dir.join("granted").exists() {
            used += 1;
            *counts.entry(request.destination).or_default() += 1;
        } else {
            pending.push((request, dir));
        }
    }
    pending.sort_by_key(|(req, dir)| (req.kind == Kind::Batch, req.issued_ns, dir.clone()));
    for (request, dir) in pending {
        if used >= limits.total {
            break;
        }
        let count = counts.entry(request.destination).or_default();
        if *count >= limits.per_destination {
            continue;
        }
        match File::create_new(dir.join("granted")) {
            Ok(_) => {
                used += 1;
                *count += 1;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

fn limits(root: &Path) -> Result<Limits, String> {
    let path = root.join("limits.json");
    let limits: Limits = if path.exists() {
        read_json(&path)?
    } else {
        Limits::default()
    };
    if !(1..=4).contains(&limits.total) || !(1..=limits.total).contains(&limits.per_destination) {
        return Err(
            "Queue limits require 1 to 4 total slots and 1 to total slots per destination.".into(),
        );
    }
    Ok(limits)
}

// Waiting clients elect exactly one scheduler per decision. The scheduler
// releases its lease immediately after the decision. Nobody polls while idle.
fn reserve(
    destination: &str,
    kind: Kind,
    mut waiting: impl FnMut(),
    mut control: impl FnMut() -> Result<bool, String>,
) -> Result<Option<Admission>, String> {
    let root = state_root()?.join("admission");
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let limits = limits(&root)?;
    let slot = allocate(&root, kind, destination)?;
    let mut announced = false;
    loop {
        if !control()? {
            return Ok(None);
        }
        if let Some(_owner) = exclusive(&root.join("scheduler.lock")).map_err(|e| e.to_string())? {
            schedule(&root, &limits)?;
        }
        if slot.dir.join("granted").is_file() {
            return Ok(control()?.then_some(slot));
        }
        if !announced {
            waiting();
            announced = true
        }
        thread::sleep(Duration::from_millis(100));
    }
}

pub(crate) fn reserve_live(
    destination: &str,
    waiting: impl FnMut(),
    mut cancelled: impl FnMut() -> bool,
) -> Result<Option<Admission>, String> {
    reserve(destination, Kind::Live, waiting, || {
        Ok(!cancelled() && !super::gui_closed()?)
    })
}

pub(super) fn reserve_request(
    destination: &str,
    context: &super::ItemContext,
) -> Result<(Admission, Instant), String> {
    let admission = reserve(
        destination,
        Kind::Batch,
        || eprintln!("Batch waiting for a request slot."),
        || context.check_control().map(|()| true),
    )?
    .ok_or("Batch admission cancelled.")?;
    Ok((admission, Instant::now() + Duration::from_secs(120)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_wins_the_next_slot_and_holds_it_between_requests() {
        let dir = std::env::temp_dir().join(id("batch_slot_test").unwrap());
        fs::create_dir(&dir).unwrap();
        let limits = Limits::default();
        let running = allocate(&dir, Kind::Batch, "local").unwrap();
        schedule(&dir, &limits).unwrap();
        let next = allocate(&dir, Kind::Batch, "local").unwrap();
        let live = allocate(&dir, Kind::Live, "local").unwrap();
        schedule(&dir, &limits).unwrap();
        assert!(!live.dir.join("granted").exists());
        drop(running);
        schedule(&dir, &limits).unwrap();
        assert!(live.dir.join("granted").exists());
        assert!(!next.dir.join("granted").exists());
        drop(live);
        schedule(&dir, &limits).unwrap();
        assert!(next.dir.join("granted").exists());
        drop(next);
        fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn abandoned_handle_releases_capacity_on_the_next_decision() {
        let dir = std::env::temp_dir().join(id("batch_stale_test").unwrap());
        fs::create_dir(&dir).unwrap();
        let limits = Limits::default();
        let mut abandoned = allocate(&dir, Kind::Live, "local").unwrap();
        schedule(&dir, &limits).unwrap();
        // A process exit closes only the OS handle. Leave its durable files.
        drop(abandoned.lease.take());
        let waiting = allocate(&dir, Kind::Batch, "local").unwrap();
        schedule(&dir, &limits).unwrap();
        assert!(!abandoned.dir.exists());
        assert!(waiting.dir.join("granted").is_file());
        drop((abandoned, waiting));
        fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn per_destination_limit_can_leave_a_slot_for_a_different_endpoint() {
        let dir = std::env::temp_dir().join(id("batch_slot_test").unwrap());
        fs::create_dir(&dir).unwrap();
        let live = allocate(&dir, Kind::Live, "a").unwrap();
        let same = allocate(&dir, Kind::Batch, "a").unwrap();
        let other = allocate(&dir, Kind::Batch, "b").unwrap();
        schedule(
            &dir,
            &Limits {
                total: 2,
                per_destination: 1,
            },
        )
        .unwrap();
        assert!(live.dir.join("granted").exists());
        assert!(other.dir.join("granted").exists());
        assert!(!same.dir.join("granted").exists());
        drop((live, same, other));
        fs::remove_dir(&dir).unwrap();
    }
}
