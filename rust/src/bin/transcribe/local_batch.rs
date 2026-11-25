//! Local completed-media workflow. All three clients use the same scoped plans and receipts.
mod policy;
mod runner;
mod storage;

use crate::model_pack;
use policy::{CANCEL, FETCH, IMPORT, PROFILE, Policy, STATUS, TRANSCRIBE};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use storage::{DirIdentity, Identity, dir_identity, id, read, valid_id, write};

pub(super) use runner::run_job;

#[derive(Debug)]
struct Failure(i32, String);
type Result<T> = std::result::Result<T, Failure>;
impl Failure {
    fn denied() -> Self {
        Self(
            3,
            "The grant does not permit this operation or resource.".into(),
        )
    }
}
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self(1, message)
    }
}
impl From<std::io::Error> for Failure {
    fn from(err: std::io::Error) -> Self {
        Self(1, err.to_string())
    }
}
fn invalid(message: &str) -> Failure {
    Failure(2, message.into())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct BatchScope {
    input: PathBuf,
    identity: Identity,
    sha256: String,
    output: PathBuf,
    output_identity: DirIdentity,
    model_revision: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
enum Operation {
    Transcribe(BatchScope),
    Import {
        source: PathBuf,
        identity: DirIdentity,
    },
}
impl Operation {
    fn group(&self) -> &'static str {
        match self {
            Self::Transcribe(_) => "batch",
            Self::Import { .. } => "profile",
        }
    }
    fn permission(&self) -> &'static str {
        match self {
            Self::Transcribe(_) => TRANSCRIBE,
            Self::Import { .. } => IMPORT,
        }
    }
    fn preview(&self) -> Value {
        match self {
            Self::Transcribe(s) => {
                json!({"operation":"transcribe","provider":"local","profile_id":PROFILE,
                "files":[s.input],"output_dir":s.output,"model_id":model_pack::MODEL_NAME,"model_revision":s.model_revision,
                "sample_rate":16000,"audio_track":"first","timestamps":true})
            }
            Self::Import { source, .. } => {
                json!({"operation":"import_model","provider":"local","source_dir":source,
                "profile_id":PROFILE,"model_id":model_pack::MODEL_NAME,"model_revision":model_pack::REVISION})
            }
        }
    }
}
#[derive(Serialize, Deserialize)]
struct Plan {
    grant: String,
    expires_at: u64,
    resource_id: String,
    operation: Operation,
    reserved_key: Option<String>,
    committed_key: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Receipt {
    plan_id: String,
}
#[derive(Debug, Serialize, Deserialize)]
struct Job {
    id: String,
    grant: String,
    scope: BatchScope,
    created_at: u64,
}
#[derive(Debug, Serialize, Deserialize)]
struct Progress {
    state: String,
    processed_frames: u64,
    segment_count: u64,
    error: Option<String>,
}
impl Default for Progress {
    fn default() -> Self {
        Self {
            state: "queued".into(),
            processed_frames: 0,
            segment_count: 0,
            error: None,
        }
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid(&format!("Supply a nonempty {key}.")))
}
fn checked_id<'a>(request: &'a Value, key: &str, prefix: &str) -> Result<&'a str> {
    let value = field(request, key)?;
    if valid_id(value, prefix) {
        Ok(value)
    } else {
        Err(invalid("Invalid resource identifier."))
    }
}
fn current_grant(root: &Path) -> Result<(String, Policy)> {
    let reference = std::env::var("AUDIO_CAPTURE_GRANT_REF").map_err(|_| Failure::denied())?;
    let policy = policy::grant(root, &reference)?;
    Ok((reference, policy))
}
fn authorized_scope(scope: &BatchScope, policy: &Policy) -> Result<()> {
    policy.allows(TRANSCRIBE)?;
    policy.input(&scope.input)?;
    policy.output(&scope.output)?;
    if scope.model_revision != model_pack::REVISION {
        return Err(invalid("Model revision changed. Preview this job again."));
    }
    Ok(())
}
fn authorize_job(job: &Job) -> std::result::Result<(), String> {
    let scope = policy::grant(&storage::root()?, &job.grant)
        .and_then(|policy| authorized_scope(&job.scope, &policy));
    scope.map_err(|failure| failure.1)
}

pub(super) fn dispatch(
    group: &str,
    action: &str,
    request: &Value,
) -> std::result::Result<Value, (i32, String)> {
    execute(group, action, request)
        .map(|mut response| {
            response["ok"] = true.into();
            response["schema_version"] = 1.into();
            response
        })
        .map_err(|failure| (failure.0, failure.1))
}

fn execute(group: &str, action: &str, req: &Value) -> Result<Value> {
    if !req.is_object() || req["schema_version"].as_u64() != Some(1) {
        return Err(invalid("Set schema_version to 1."));
    }
    if (group, action) == ("diagnostics", "capabilities") {
        return Ok(
            json!({"profile_id":PROFILE,"provider":"local","file_types":["mp4"],"sample_rate":16000,
            "max_files":1,"max_active_jobs":MAX_ACTIVE_JOBS,"page_limit":100,"model_revision":model_pack::REVISION,
            "operations":{"batch":["preview","commit","status","list","cancel"],"transcript":["fetch"],
                "profile":["model_list","preview","commit"],"diagnostics":["capabilities"]}}),
        );
    }
    if (group, action) == ("profile", "model_list") {
        return Ok(
            json!({"models":[{"model_id":model_pack::MODEL_NAME,"revision":model_pack::REVISION,
            "installed":model_pack::installed(),"profile_id":PROFILE,"provider":"local","language":"en",
            "format":model_pack::MODEL_FORMAT,"license":model_pack::MODEL_LICENSE,
            "source":model_pack::MODEL_SOURCE,"weights_sha256":model_pack::WEIGHTS_SHA256}]}),
        );
    }
    let root = storage::root()?;
    match (group, action) {
        ("grants", "provision") => {
            if std::env::var_os("AUDIO_CAPTURE_GRANT_REF").is_some() {
                return Err(Failure::denied());
            }
            let policy:Policy = serde_json::from_value(req["policy"].clone())
                .map_err(|_| invalid("Invalid grant policy. Supply explicit input roots, output roots, operations, profiles, and approval permission."))?;
            let policy = policy.validate()?;
            let reference = id("grant")?;
            write(&root.join("grants").join(&reference), &policy)?;
            Ok(json!({"grant_ref":reference}))
        }
        ("grants", "revoke") => {
            if std::env::var_os("AUDIO_CAPTURE_GRANT_REF").is_some() {
                return Err(Failure::denied());
            }
            let reference = checked_id(req, "grant_ref", "grant")?;
            let _lock = storage::lock(&root.join("control.lock"))?;
            fs::remove_file(root.join("grants").join(reference)).map_err(|_| Failure::denied())?;
            Ok(json!({"state":"revoked"}))
        }
        ("profile" | "batch", "preview") => preview(&root, group, req),
        ("profile" | "batch", "commit") => commit(&root, group, req),
        ("batch", "list") => list_jobs(&root, req),
        ("batch", "status" | "cancel") | ("transcript", "fetch") => {
            access_job(&root, group, action, req)
        }
        _ => Err(invalid("Unsupported local control operation.")),
    }
}

fn preview(root: &Path, group: &str, req: &Value) -> Result<Value> {
    let (grant, policy) = current_grant(root)?;
    let operation = match (group, field(req, "operation")?) {
        ("batch", "transcribe") => {
            policy.allows(TRANSCRIBE)?;
            if req["profile_id"].as_str() != Some(PROFILE) {
                return Err(invalid("Select the local-whisper profile."));
            }
            let file = req["files"]
                .as_array()
                .filter(|v| v.len() == 1)
                .and_then(|v| v[0].as_str())
                .ok_or_else(|| invalid("Select one completed MP4."))?;
            policy.requested_input(Path::new(file))?;
            let (mut input, path, identity) = storage::open_input(Path::new(file))
                .map_err(|_| invalid("Choose a readable, finished local MP4."))?;
            policy.input(&path)?;
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
            {
                return Err(invalid("Choose an MP4 file."));
            }
            if !model_pack::installed() {
                return Err(invalid(
                    "Import the verified base.en model before you transcribe.",
                ));
            }
            let output = match req.get("output_dir") {
                None => policy
                    .output_roots
                    .first()
                    .ok_or_else(Failure::denied)?
                    .as_path(),
                Some(v) => Path::new(
                    v.as_str()
                        .ok_or_else(|| invalid("Choose a local output directory."))?,
                ),
            };
            policy.requested_output(output)?;
            let (output_handle, output) = storage::hold_dir(output)
                .map_err(|_| invalid("Choose an existing local output directory."))?;
            policy.output(&output)?;
            let sha256 = crate::jobs::digest(&mut input)?;
            Operation::Transcribe(BatchScope {
                input: path,
                identity,
                sha256,
                output,
                output_identity: dir_identity(&output_handle)?,
                model_revision: model_pack::REVISION.into(),
            })
        }
        ("profile", "import_model") => {
            policy.allows(IMPORT)?;
            let source = Path::new(field(req, "source_dir")?);
            policy.requested_input(source)?;
            let (dir, source) = storage::hold_dir(source)
                .map_err(|_| invalid("Choose the folder that contains the model pack."))?;
            policy.input(&source)?;
            Operation::Import {
                source,
                identity: dir_identity(&dir)?,
            }
        }
        _ => return Err(invalid("Unsupported preview operation.")),
    };
    let plan_id = id("plan")?;
    let plan = Plan {
        grant,
        expires_at: now().saturating_add(600),
        resource_id: id("batch")?,
        operation,
        reserved_key: None,
        committed_key: None,
    };
    write(&root.join("plans").join(&plan_id), &plan)?;
    Ok(json!({"plan_id":plan_id,"scope":plan.operation.preview(),"expires_at":plan.expires_at}))
}

fn key_name(key: &str) -> Result<String> {
    if key.is_empty()
        || key.len() > 96
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(invalid(
            "Use 1 to 96 letters, digits, periods, hyphens, or underscores for idempotency_key.",
        ));
    }
    crate::jobs::digest(key.as_bytes()).map_err(Into::into)
}

fn prepare_operation(operation: &Operation, policy: &Policy) -> Result<Vec<File>> {
    match operation {
        Operation::Transcribe(scope) => {
            authorized_scope(scope, policy)?;
            let (mut file, actual, identity) = storage::open_input(&scope.input)?;
            let (folder, destination) = storage::hold_dir(&scope.output)?;
            if actual != scope.input
                || destination != scope.output
                || dir_identity(&folder)? != scope.output_identity
                || identity != scope.identity
                || crate::jobs::digest(&mut file)? != scope.sha256
            {
                return Err(invalid(
                    "An approved input or output changed. Request a new preview.",
                ));
            }
            if !model_pack::installed() {
                return Err(invalid("Import the approved local model before you start."));
            }
            Ok(vec![file, folder])
        }
        Operation::Import { source, identity } => {
            let (dir, actual) = storage::hold_dir(source)?;
            policy.input(&actual)?;
            if &actual != source || &dir_identity(&dir)? != identity {
                return Err(invalid("The model source changed. Preview it again."));
            }
            Ok(vec![dir])
        }
    }
}

const MAX_ACTIVE_JOBS: usize = 4;
fn reserve_slot(root: &Path, batch_id: &str) -> Result<()> {
    // Only the admission transaction holds control.lock. Reclaim terminal slots
    // in constant time, independently of the number of historical transcripts.
    let mut available = None;
    for index in 0..MAX_ACTIVE_JOBS {
        let slot = root.join(format!("slot-{index}.json"));
        if !slot.is_file() {
            available.get_or_insert(slot);
            continue;
        }
        let occupant: String = read(&slot)?;
        if occupant == batch_id {
            return Ok(());
        }
        if !valid_id(&occupant, "batch") {
            return Err(invalid("Invalid local admission record."));
        }
        let dir = root.join("jobs").join(occupant);
        let path = dir.join("job.json");
        if path.is_file() {
            let Some(_mutation) = storage::try_lock(&dir.join("mutate.lock"))? else {
                continue;
            };
            let prior: Job = read(&path)?;
            if matches!(
                progress(&dir, &prior)?.state.as_str(),
                "queued" | "running" | "publishing"
            ) {
                continue;
            }
        }
        available.get_or_insert(slot);
    }
    let slot = available.ok_or_else(|| {
        Failure(
            1,
            "The local queue has four active jobs. Retry the approved plan when a slot is free."
                .into(),
        )
    })?;
    write(&slot, &batch_id)?;
    Ok(())
}

/// The caller can lose its response after a durable job exists. Acknowledge the
/// same approved job even if its source, model, or preview has since expired.
fn accepted_batch(root: &Path, plan: &Plan, plan_id: &str, key: &str) -> Result<bool> {
    let Operation::Transcribe(scope) = &plan.operation else {
        return Ok(false);
    };
    if plan.reserved_key.as_deref() != Some(key) {
        return Ok(false);
    }
    let receipt_path = root.join("receipts").join(format!("{}-{key}", plan.grant));
    let job_dir = root.join("jobs").join(&plan.resource_id);
    let job_path = job_dir.join("job.json");
    if !receipt_path.is_file() || !job_path.is_file() {
        return Ok(false);
    }
    let receipt: Receipt = read(&receipt_path)?;
    let job: Job = read(&job_path)?;
    if receipt.plan_id != plan_id
        || job.id != plan.resource_id
        || job.grant != plan.grant
        || job.scope != *scope
    {
        return Err(invalid(
            "The approval receipt does not match this local job.",
        ));
    }
    let progress: Progress = read(&job_dir.join("progress.json"))?;
    // A still-valid plan may repair the narrow gap before dispatch if no worker
    // owns the queue. After expiry, acknowledge the durable job without starting it.
    Ok(progress.state != "queued"
        || plan.expires_at < now()
        || storage::try_lock(&job_dir.join("running.lock"))?.is_none())
}

fn commit(root: &Path, group: &str, req: &Value) -> Result<Value> {
    if req["approved"].as_bool() != Some(true) {
        return Err(invalid("Commit requires explicit approval."));
    }
    let plan_id = checked_id(req, "plan_id", "plan")?;
    let key = key_name(field(req, "idempotency_key")?)?;
    let (reference, policy) = current_grant(root)?;
    let path = root.join("plans").join(plan_id);
    let mut plan: Plan = read(&path).map_err(|_| Failure::denied())?;
    if plan.grant != reference || plan.operation.group() != group || !policy.allow_headless_approval
    {
        return Err(Failure::denied());
    }
    policy.allows(plan.operation.permission())?;
    // Long reads hold the per-plan lease. Other grants can still commit, revoke, or poll.
    let _plan_lock = storage::lock(&path.with_extension("lock"))?;
    plan = read(&path)?;
    if plan
        .reserved_key
        .as_ref()
        .is_some_and(|previous| previous != &key)
    {
        return Err(invalid("This preview already has an approval key."));
    }
    if plan.committed_key.is_some() {
        return Ok(commit_result(&plan));
    }
    if accepted_batch(root, &plan, plan_id, &key)? {
        let _admission = storage::lock(&root.join("control.lock"))?;
        policy::grant(root, &reference)?.allows(plan.operation.permission())?;
        plan.committed_key = Some(key);
        write(&path, &plan)?;
        return Ok(commit_result(&plan));
    }
    if plan.expires_at < now() {
        return Err(invalid("The preview expired. Request a new preview."));
    }
    let _approved_handles = prepare_operation(&plan.operation, &policy)?;
    let admission = storage::lock(&root.join("control.lock"))?;
    // Revocation and acceptance serialize here, after checking the held media.
    policy::grant(root, &reference)?.allows(plan.operation.permission())?;
    if plan.expires_at < now() {
        return Err(invalid("The preview expired. Request a new preview."));
    }
    let receipt_path = root.join("receipts").join(format!("{reference}-{key}"));
    if receipt_path.is_file() {
        let receipt: Receipt = read(&receipt_path)?;
        if receipt.plan_id != plan_id {
            return Err(invalid(
                "The idempotency key already belongs to another preview.",
            ));
        }
    }
    if matches!(plan.operation, Operation::Transcribe(_)) {
        reserve_slot(root, &plan.resource_id)?;
    }
    if plan.reserved_key.is_none() {
        plan.reserved_key = Some(key.clone());
        write(&path, &plan)?;
    }
    write(
        &receipt_path,
        &Receipt {
            plan_id: plan_id.into(),
        },
    )?;
    match &plan.operation {
        Operation::Transcribe(scope) => {
            let jobdir = root.join("jobs").join(&plan.resource_id);
            if !jobdir.is_dir() {
                fs::create_dir(&jobdir)?;
            }
            let job_path = jobdir.join("job.json");
            if !job_path.is_file() {
                let job = Job {
                    id: plan.resource_id.clone(),
                    grant: reference,
                    scope: scope.clone(),
                    created_at: now(),
                };
                write(&jobdir.join("progress.json"), &Progress::default())?;
                write(&job_path, &job)?;
            }
            drop(admission);
        }
        Operation::Import { source, .. } => {
            drop(admission);
            model_pack::install(source)?;
        }
    }
    if let Operation::Transcribe(_) = &plan.operation {
        let dir = root.join("jobs").join(&plan.resource_id);
        if let Err(err) = launch(&plan.resource_id) {
            write(
                &dir.join("progress.json"),
                &Progress {
                    state: "failed".into(),
                    error: Some(err.to_string()),
                    ..Default::default()
                },
            )?;
        }
    }
    // Persist the final receipt after dispatch. A retry reuses the same job; its OS lease prevents duplicate inference.
    plan.committed_key = Some(key);
    write(&path, &plan)?;
    Ok(commit_result(&plan))
}
fn commit_result(plan: &Plan) -> Value {
    match &plan.operation {
        Operation::Transcribe(_) => json!({"batch_id":plan.resource_id,"state":"accepted"}),
        Operation::Import { .. } => {
            json!({"model_id":model_pack::MODEL_NAME,"revision":model_pack::REVISION,"state":"installed"})
        }
    }
}
fn launch(job_id: &str) -> std::io::Result<()> {
    use std::ffi::OsString;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
        PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW,
    };

    let executable = std::env::current_exe()?;
    let application: Vec<_> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // Job IDs contain only the validated prefix and hex bytes. Windows filenames cannot contain quotes.
    let mut command = OsString::from("\"");
    command.push(&executable);
    command.push("\" --local-job ");
    command.push(job_id);
    let mut command: Vec<_> = command.encode_wide().chain(Some(0)).collect();

    // A durable worker uses no caller pipe or credential environment. The parent can
    // return EOF to an MCP or CLI client even while this process is transcribing.
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().div_ceil(2).min(4));
    let mut environment = Vec::new();
    for name in [
        "AUDIO_CAPTURE_STATE_DIR",
        "LOCALAPPDATA",
        "RAYON_NUM_THREADS",
        "SYSTEMROOT",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "WINDIR",
    ] {
        let value = if name == "RAYON_NUM_THREADS" {
            Some(OsString::from(threads.to_string()))
        } else {
            std::env::var_os(name)
        };
        if let Some(value) = value {
            environment.extend(name.encode_utf16());
            environment.push(u16::from(b'='));
            environment.extend(value.encode_wide());
            environment.push(0);
        }
    }
    environment.push(0);
    let mut startup: STARTUPINFOW = unsafe { zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    startup.dwFlags = STARTF_USESTDHANDLES;
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | BELOW_NORMAL_PRIORITY_CLASS,
            environment.as_ptr().cast(),
            null(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        return Err(std::io::Error::last_os_error());
    }
    unsafe {
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    Ok(())
}
fn progress(dir: &Path, job: &Job) -> Result<Progress> {
    let path = dir.join("progress.json");
    let mut p: Progress = read(&path)?;
    // Allow a newly created worker to enter main. OS lease release detects a terminated owner.
    if matches!(p.state.as_str(), "queued" | "running" | "publishing")
        && now().saturating_sub(job.created_at) > 30
        && let Some(_abandoned) = storage::try_lock(&dir.join("running.lock"))?
    {
        if p.state == "publishing" && runner::has_publication(dir, job) {
            p.state = "completed".into();
        } else {
            p.state = "interrupted".into();
            p.error = Some(
                "The local worker stopped. Review partial text or preview another transcription."
                    .into(),
            );
        }
        write(&path, &p)?;
    }
    Ok(p)
}

fn public_status(job: &Job, p: &Progress, dir: &Path) -> Value {
    json!({"batch_id":job.id,"state":p.state,"processed_frames":p.processed_frames,
        "sample_rate":16000,"segment_count":p.segment_count,"error":p.error,
        "cancel_requested":dir.join("cancel").exists()})
}

fn access_job(root: &Path, group: &str, action: &str, req: &Value) -> Result<Value> {
    let (reference, policy) = current_grant(root)?;
    policy.allows(match (group, action) {
        ("batch", "cancel") => CANCEL,
        ("transcript", "fetch") => FETCH,
        _ => STATUS,
    })?;
    let batch_id = checked_id(req, "batch_id", "batch")?;
    let dir = root.join("jobs").join(batch_id);
    let job: Job = read(&dir.join("job.json")).map_err(|_| Failure::denied())?;
    if job.grant != reference {
        return Err(Failure::denied());
    }
    let _mutation = storage::lock(&dir.join("mutate.lock"))?;
    let p = progress(&dir, &job)?;
    if action == "cancel"
        && matches!(p.state.as_str(), "queued" | "running")
        && !dir.join("cancel").exists()
    {
        File::create_new(dir.join("cancel"))?.sync_all()?;
    }
    drop(_mutation);
    if action == "fetch" {
        page(req)?;
        return runner::fetch(&job, req).map_err(Into::into);
    }
    Ok(public_status(&job, &p, &dir))
}

fn list_jobs(root: &Path, req: &Value) -> Result<Value> {
    let (reference, policy) = current_grant(root)?;
    policy.allows(STATUS)?;
    let (offset, limit) = page(req)?;
    let mut ids = Vec::new();
    for entry in fs::read_dir(root.join("jobs"))? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name.to_str().filter(|id| valid_id(id, "batch")) else {
            continue;
        };
        if read::<Job>(&entry.path().join("job.json")).is_ok_and(|j| j.grant == reference) {
            ids.push(id.to_string());
        }
    }
    ids.sort_unstable();
    let end = offset.saturating_add(limit).min(ids.len());
    Ok(
        json!({"batch_ids":ids.get(offset..end).unwrap_or_default(),"next_offset":(end<ids.len()).then_some(end)}),
    )
}
fn page(req: &Value) -> Result<(usize, usize)> {
    let get = |name: &str, default: usize| match req.get(name) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| invalid("Use a nonnegative integer page offset and limit.")),
    };
    let offset = get("offset", 0)?;
    let limit = get("limit", 50)?;
    if !(1..=100).contains(&limit) {
        return Err(invalid("Use a page limit from 1 to 100."));
    }
    Ok((offset, limit))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_unversioned_requests_and_unapproved_commits() {
        assert_eq!(
            dispatch("diagnostics", "capabilities", &json!({}))
                .unwrap_err()
                .0,
            2
        );
        assert!(key_name("../other-user").is_err());
        assert_ne!(key_name("Aa").unwrap(), key_name("aa").unwrap());
        assert!(commit(Path::new("."), "batch", &json!({"approved":false})).is_err());
    }
    #[test]
    fn advertises_only_implemented_offline_operations_and_bounded_pages() {
        let result = dispatch("diagnostics", "capabilities", &json!({"schema_version":1})).unwrap();
        assert_eq!(result["file_types"], json!(["mp4"]));
        assert_eq!(result["ok"], true);
        assert!(page(&json!({"limit":101})).is_err());
        assert!(page(&json!({"offset":-1})).is_err());
    }
    // Build private synthetic approvals without touching the operator's model or grant store.
    fn fixture_scope(root: &Path) -> BatchScope {
        let input = root.join("fixture.mp4");
        fs::write(&input, b"synthetic stable file").unwrap();
        let (mut file, input, identity) = storage::open_input(&input).unwrap();
        let output = root.join("out");
        fs::create_dir(&output).unwrap();
        let (folder, output) = storage::hold_dir(&output).unwrap();
        BatchScope {
            input,
            identity,
            sha256: crate::jobs::digest(&mut file).unwrap(),
            output,
            output_identity: dir_identity(&folder).unwrap(),
            model_revision: model_pack::REVISION.into(),
        }
    }
    fn workspace() -> PathBuf {
        let root = std::env::temp_dir().join(id("local_batch_test").unwrap());
        fs::create_dir(&root).unwrap();
        root
    }
    #[test]
    fn reject_a_replacement_at_the_approved_output_path() {
        let root = workspace();
        let scope = fixture_scope(&root);
        let policy = Policy {
            input_roots: vec![scope.input.clone()],
            output_roots: vec![scope.output.clone()],
            input_spellings: vec![],
            output_spellings: vec![],
            operations: vec![TRANSCRIBE.into()],
            profiles: vec![PROFILE.into()],
            allow_headless_approval: true,
        };
        fs::rename(&scope.output, root.join("previous")).unwrap();
        fs::create_dir(&scope.output).unwrap();
        let err = prepare_operation(&Operation::Transcribe(scope), &policy).unwrap_err();
        assert!(err.1.contains("changed"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn admission_holds_four_active_jobs_and_recycles_a_released_slot() {
        let root = workspace();
        fs::create_dir(root.join("jobs")).unwrap();
        let scope = fixture_scope(&root);
        let mut ids = Vec::new();
        for _ in 0..MAX_ACTIVE_JOBS {
            let batch_id = id("batch").unwrap();
            reserve_slot(&root, &batch_id).unwrap();
            let dir = root.join("jobs").join(&batch_id);
            fs::create_dir(&dir).unwrap();
            write(
                &dir.join("job.json"),
                &Job {
                    id: batch_id.clone(),
                    grant: "fixture".into(),
                    scope: scope.clone(),
                    created_at: now(),
                },
            )
            .unwrap();
            write(&dir.join("progress.json"), &Progress::default()).unwrap();
            ids.push(batch_id);
        }
        let waiting = id("batch").unwrap();
        assert!(reserve_slot(&root, &waiting).is_err());
        reserve_slot(&root, &ids[0]).unwrap();
        write(
            &root.join("jobs").join(&ids[0]).join("progress.json"),
            &Progress {
                state: "cancelled".into(),
                ..Default::default()
            },
        )
        .unwrap();
        reserve_slot(&root, &waiting).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn lexical_scope_has_one_denial_for_existing_and_absent_outside_paths() {
        let root = workspace();
        let scope = fixture_scope(&root);
        let unrelated = root.join("other.mp4");
        fs::write(&unrelated, b"unrelated").unwrap();
        let input = root.join("FIXTURE.MP4");
        let output = root.join("OUT");
        let policy = Policy {
            input_roots: vec![input.clone()],
            output_roots: vec![output.clone()],
            input_spellings: vec![unrelated.clone()],
            output_spellings: vec![root.clone()],
            operations: vec![TRANSCRIBE.into()],
            profiles: vec![PROFILE.into()],
            allow_headless_approval: true,
        }
        .validate()
        .unwrap();
        for path in [
            unrelated,
            root.join("missing.mp4"),
            root.join("out").join("..").join("sibling"),
        ] {
            let input = policy.requested_input(&path).unwrap_err();
            let output = policy.requested_output(&path).unwrap_err();
            assert_eq!(input.0, 3);
            assert_eq!((input.0, input.1), (output.0, output.1));
        }
        assert!(policy.requested_input(&input).is_ok());
        assert!(policy.requested_output(&output).is_ok());
        assert!(policy.requested_input(&scope.input).is_ok());
        assert!(policy.requested_output(&scope.output).is_ok());
        assert!(policy.input(&scope.input).is_ok());
        assert!(policy.output(&scope.output).is_ok());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn expired_receipt_recovers_only_the_exact_approved_job_after_media_is_gone() {
        let root = workspace();
        let scope = fixture_scope(&root);
        let ref_id = id("grant").unwrap();
        let plan_id = id("plan").unwrap();
        let key = key_name("key-1").unwrap();
        let batch = id("batch").unwrap();
        let mut plan = Plan {
            grant: ref_id.clone(),
            expires_at: now() - 1,
            resource_id: batch.clone(),
            operation: Operation::Transcribe(scope.clone()),
            reserved_key: Some(key.clone()),
            committed_key: None,
        };
        let dir = root.join("jobs").join(&batch);
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir(root.join("receipts")).unwrap();
        let receipt_path = root.join("receipts").join(format!("{ref_id}-{key}"));
        write(
            &receipt_path,
            &Receipt {
                plan_id: plan_id.clone(),
            },
        )
        .unwrap();
        assert!(!accepted_batch(&root, &plan, &plan_id, &key).unwrap());
        let job = Job {
            id: batch,
            grant: ref_id,
            scope: scope.clone(),
            created_at: now() - 650,
        };
        write(&dir.join("job.json"), &job).unwrap();
        write(&dir.join("progress.json"), &Progress::default()).unwrap();
        fs::remove_file(&scope.input).unwrap();
        assert!(accepted_batch(&root, &plan, &plan_id, &key).unwrap());
        assert!(!accepted_batch(&root, &plan, &plan_id, "wrong-key").unwrap());
        assert!(accepted_batch(&root, &plan, "wrong-plan", &key).is_err());
        plan.expires_at = now() + 600;
        assert!(!accepted_batch(&root, &plan, &plan_id, &key).unwrap());
        write(
            &dir.join("progress.json"),
            &Progress {
                state: "running".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(accepted_batch(&root, &plan, &plan_id, &key).unwrap());
        plan.operation = Operation::Transcribe(BatchScope {
            sha256: "different".into(),
            ..scope
        });
        assert!(accepted_batch(&root, &plan, &plan_id, &key).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
