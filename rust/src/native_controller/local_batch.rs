//! Native controls for a completed local MP4. Every service request runs off the window thread.
use super::*;
use crate::worker_launch::{EndpointSession, control};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Duration,
};

const PROFILE: &str = "local-whisper";
const PAGE: u64 = 100;
const STATUS_PERIOD: Duration = Duration::from_secs(2);

struct Plan {
    grant: String,
    id: String,
    file: String,
    output: String,
    description: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Receipt {
    grant: String,
    batch: String,
}

enum Reply {
    Model(bool),
    Imported,
    Preview(Plan),
    Accepted(Receipt),
    Status(Value),
    Page(Value, u64),
    Reopened(Option<Receipt>),
}

#[derive(Default)]
pub(super) struct Panel {
    opened: bool,
    visible: bool,
    busy: Option<Receiver<Result<Reply, String>>>,
    plan: Option<Plan>,
    job: Option<Receipt>,
    polling: bool,
    approving: bool,
    cancel_requested: bool,
    check: Option<Instant>,
    offset: u64,
    next: Option<u64>,
    endpoint: Option<EndpointSession>,
    log: Vec<u8>,
}

fn object_field<'a>(obj: &'a Value, key: &str) -> Result<&'a str, String> {
    obj[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Missing worker field {key}."))
}

fn request(
    group: &str,
    action: &str,
    grant: Option<&str>,
    mut body: Value,
) -> Result<Value, String> {
    body["schema_version"] = 1.into();
    control(group, action, grant, body)
}

fn grant(input: &Path, output: Option<&Path>, operations: &[&str]) -> Result<String, String> {
    let result = request(
        "grants",
        "provision",
        None,
        json!({"policy":{
            "input_roots":[input], "output_roots":output.into_iter().collect::<Vec<_>>(),
            "profiles":[PROFILE],"operations":operations,"allow_headless_approval":true
        }}),
    )?;
    Ok(object_field(&result, "grant_ref")?.into())
}

fn revoke(grant: &str) {
    let _ = request("grants", "revoke", None, json!({"grant_ref":grant}));
}

fn commit(group: &str, grant: &str, plan_id: &str) -> Result<Value, String> {
    request(
        group,
        "commit",
        Some(grant),
        json!({
            "plan_id":plan_id, "approved":true,"idempotency_key":plan_id
        }),
    )
}

fn preview(file: String, output: String) -> Result<Reply, String> {
    let access = grant(
        Path::new(&file),
        Some(Path::new(&output)),
        &[
            "batch.transcribe",
            "batch.status",
            "batch.cancel",
            "transcript.fetch",
        ],
    )?;
    let result = (|| {
        let reply = request(
            "batch",
            "preview",
            Some(&access),
            json!({
                "operation":"transcribe","profile_id":PROFILE,"files":[file],"output_dir":output
            }),
        )?;
        let scope = &reply["scope"];
        let source = scope["files"]
            .as_array()
            .and_then(|fs| fs.first())
            .and_then(Value::as_str)
            .ok_or("The worker returned no file in the approved scope.")?;
        let output_dir = object_field(scope, "output_dir")?;
        let revision = object_field(scope, "model_revision")?;
        if scope["provider"] != "local" || scope["profile_id"] != PROFILE {
            return Err("The worker did not select the local speech profile.".into());
        }
        let description = format!(
            "Ready to transcribe locally with Whisper base.en ({:.12}).\r\nFirst audio track: {source}\r\nSave text and timed JSON in: {output_dir}",
            revision
        );
        Ok(Reply::Preview(Plan {
            grant: access.clone(),
            id: object_field(&reply, "plan_id")?.into(),
            file,
            output,
            description,
        }))
    })();
    if result.is_err() {
        revoke(&access);
    }
    result
}

fn install(dir: String) -> Result<Reply, String> {
    let path = Path::new(&dir);
    let access = grant(path, None, &["model.import"])?;
    let result = (|| {
        let plan = request(
            "profile",
            "preview",
            Some(&access),
            json!({"operation":"import_model","source_dir":path}),
        )?;
        commit("profile", &access, object_field(&plan, "plan_id")?)?;
        Ok(Reply::Imported)
    })();
    revoke(&access);
    result
}

fn state_dir() -> Result<PathBuf, String> {
    let root = std::env::var_os("AUDIO_CAPTURE_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(|p| PathBuf::from(p).join("AudioCaptureRust/native-control-v1"))
        })
        .ok_or("Cannot find the local job state folder.")?;
    Ok(root.join("receipts"))
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum SavedJob {
    Approved { grant: String, plan: String },
    Accepted { grant: String, batch: String },
}

fn receipt_path() -> Result<PathBuf, String> {
    let dir = state_dir()?;
    // The service already verified and created its private receipts directory during preview.
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    Ok(dir.join(format!("ui-{time:032}-{}.json", std::process::id())))
}

fn remember(path: &Path, saved: &SavedJob) -> Result<(), String> {
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| format!("Cannot save the local job reference. {e}"))?;
    let result = (|| {
        serde_json::to_writer(&mut file, saved).map_err(|e| e.to_string())?;
        file.flush()
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&tmp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

fn submit_approved(path: &Path, access: String, id: &str) -> Result<Receipt, String> {
    let reply = commit("batch", &access, id)?;
    let batch = object_field(&reply, "batch_id")?.to_owned();
    // The approved record can replay this same intent if the reply or this write is lost.
    remember(
        path,
        &SavedJob::Accepted {
            grant: access.clone(),
            batch: batch.clone(),
        },
    )?;
    Ok(Receipt {
        grant: access,
        batch,
    })
}

fn reopen() -> Result<Reply, String> {
    let entries = match fs::read_dir(state_dir()?) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Reply::Reopened(None)),
        Err(e) => return Err(e.to_string()),
    };
    let mut latest = None;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|name| name.starts_with("ui-") && name.ends_with(".json"))
            && latest.as_ref().is_none_or(|p| &path > p)
        {
            latest = Some(path);
        }
    }
    let Some(path) = latest else {
        return Ok(Reply::Reopened(None));
    };
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .and_then(|f| f.take(4097).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4096 {
        return Err("Invalid saved job reference.".into());
    }
    let saved = serde_json::from_slice::<SavedJob>(&bytes)
        .map_err(|_| "Cannot read the saved local job reference.")?;
    let job = match saved {
        SavedJob::Approved { grant, plan } => submit_approved(&path, grant, &plan)?,
        SavedJob::Accepted { grant, batch } => Receipt { grant, batch },
    };
    Ok(Reply::Reopened(Some(job)))
}

fn model_status() -> Result<Reply, String> {
    let response = request("profile", "model_list", None, json!({}))?;
    let installed = response["models"]
        .as_array()
        .ok_or("Invalid model list.")?
        .iter()
        .any(|m| m["installed"] == true);
    Ok(Reply::Model(installed))
}

fn format_page(response: &Value) -> Result<String, String> {
    let entries = response["segments"]
        .as_array()
        .ok_or("Missing timed segments.")?;
    let mut lines = Vec::with_capacity(entries.len());
    let timestamp = |ms: u64| format!("{}.{:03}", clock(ms / 1000), ms % 1000);
    for entry in entries {
        let start = entry["start_ms"].as_u64().ok_or("Missing start time.")?;
        let end = entry["end_ms"].as_u64().ok_or("Missing end time.")?;
        lines.push(format!(
            "[{} - {}] {}",
            timestamp(start),
            timestamp(end),
            object_field(entry, "text")?.trim()
        ));
    }
    Ok(lines.join("\r\n"))
}

impl Panel {
    pub(super) fn needs_tick(&self) -> bool {
        self.busy.is_some() || (self.visible && self.polling) || self.endpoint.is_some()
    }

    pub(super) fn approving(&self) -> bool {
        self.approving
    }

    fn spawn(&mut self, job: impl FnOnce() -> Result<Reply, String> + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Err(mpsc::SendError(Ok(Reply::Preview(plan)))) = tx.send(job()) {
                revoke(&plan.grant);
            }
        });
        self.busy = Some(rx);
    }

    fn editable(&self) -> bool {
        self.busy.is_none() && !self.polling && self.endpoint.is_none()
    }

    fn controls(&self, view: &View) {
        if !view.visible(LOCAL_INPUT) {
            return;
        }
        let interactive = self.endpoint.is_some();
        for id in [
            ENDPOINT_REPLY_LABEL,
            ENDPOINT_REPLY,
            ENDPOINT_SEND,
            ENDPOINT_CLOSE,
        ] {
            view.set_visible(id, interactive);
        }
        let editable = self.editable();
        for id in [
            LOCAL_INPUT,
            LOCAL_INPUT_BROWSE,
            LOCAL_OUTPUT,
            LOCAL_OUTPUT_BROWSE,
            LOCAL_PREVIEW,
            LOCAL_MODEL,
            LOCAL_MODEL_BROWSE,
            LOCAL_IMPORT,
            ENDPOINT_FILES,
            ENDPOINT_QUEUE,
        ] {
            enabled(view, id, editable);
        }
        enabled(view, LOCAL_APPROVE, editable && self.plan.is_some());
        enabled(view, LOCAL_CANCEL, self.polling && self.job.is_some());
        enabled(view, LOCAL_PREVIOUS, editable && self.offset > 0);
        enabled(view, LOCAL_NEXT, editable && self.next.is_some());
    }

    pub(super) fn panel(&mut self, view: &View, open: bool, current: bool) {
        self.visible = open;
        if !open {
            return;
        }
        if !self.opened {
            self.opened = true;
            let configured = text(view, LIVE_OUTPUT_DIR);
            let output = if configured.trim().is_empty() {
                text(view, FOLDER)
            } else {
                configured
            };
            set_text(view, LOCAL_OUTPUT, &output);
            self.spawn(reopen);
        } else if current && self.editable() {
            self.discard_plan();
            self.next = None;
            self.offset = 0;
            self.spawn(reopen);
        }
        self.controls(view);
    }

    fn discard_plan(&mut self) {
        if let Some(old) = self.plan.take() {
            std::thread::spawn(move || revoke(&old.grant));
        }
    }

    fn changed(&mut self, view: &View) {
        if self.plan.is_some() {
            self.discard_plan();
            set_text(
                view,
                LOCAL_STATUS,
                "Selection changed. Preview the new local job.",
            );
        }
        self.controls(view);
    }

    fn start_preview(&mut self, view: &View) {
        let file = text(view, LOCAL_INPUT).trim().to_owned();
        let output = text(view, LOCAL_OUTPUT).trim().to_owned();
        if file.is_empty() || output.is_empty() {
            set_text(
                view,
                LOCAL_STATUS,
                "Choose an MP4 file and an existing output folder.",
            );
            return;
        }
        self.discard_plan();
        self.next = None;
        self.offset = 0;
        set_text(
            view,
            LOCAL_STATUS,
            "Checking the selected file, output folder and local model…",
        );
        set_text(view, LOCAL_REVIEW, "");
        self.spawn(move || preview(file, output));
    }

    fn accept(&mut self, view: &View) {
        let Some(plan) = self.plan.as_ref() else {
            return;
        };
        if plan.file != text(view, LOCAL_INPUT).trim()
            || plan.output != text(view, LOCAL_OUTPUT).trim()
        {
            self.changed(view);
            return;
        }
        // Record the approval before submitting it. The same key can recover a lost receipt.
        let plan = self.plan.take().expect("verified preview");
        self.approving = true;
        self.spawn(move || {
            let prepared = (|| {
                let path = receipt_path()?;
                remember(
                    &path,
                    &SavedJob::Approved {
                        grant: plan.grant.clone(),
                        plan: plan.id.clone(),
                    },
                )?;
                Ok::<_, String>(path)
            })();
            let path = match prepared {
                Ok(path) => path,
                Err(e) => {
                    revoke(&plan.grant);
                    return Err(e);
                }
            };
            submit_approved(&path, plan.grant, &plan.id).map(Reply::Accepted)
        });
        set_text(view, LOCAL_STATUS, "Submitting the approved local job…");
    }

    fn fetch(&mut self, offset: u64) {
        if let Some(job) = self.job.clone() {
            self.spawn(move || {
                request(
                    "transcript",
                    "fetch",
                    Some(&job.grant),
                    json!({"batch_id":job.batch,"offset":offset,"limit":PAGE}),
                )
                .map(|page| Reply::Page(page, offset))
            });
        }
    }

    fn endpoint(&mut self, view: &View, selected: bool) {
        let files = if selected {
            let Some(files) = rfd::FileDialog::new()
                .set_title("Select files for the configured endpoint")
                .add_filter(
                    "Audio and video files",
                    audio_capture::TRANSCRIPTION_EXTENSIONS,
                )
                .add_filter("Finalized recording sessions", &["jsonl"])
                .pick_files()
            else {
                return;
            };
            Some(files)
        } else {
            None
        };
        self.discard_plan();
        let config = super::live::batch_transcription_config(view);
        match EndpointSession::start(files.as_deref(), config.as_ref()) {
            Ok(session) => {
                self.log.clear();
                self.endpoint = Some(session);
                self.next = None;
                self.offset = 0;
                set_text(view, LOCAL_REVIEW, "");
                set_text(
                    view,
                    LOCAL_STATUS,
                    "Endpoint session. Read each question and use Send reply.",
                );
            }
            Err(e) => set_text(view, LOCAL_STATUS, &e),
        }
    }

    pub(super) fn command(&mut self, view: &View, id: i32, notification: u16) {
        if !view.visible(LOCAL_INPUT) {
            return;
        }
        if notification == EN_CHANGE as u16 && matches!(id, LOCAL_INPUT | LOCAL_OUTPUT) {
            self.changed(view);
            return;
        }
        if notification != BN_CLICKED {
            return;
        }
        if id == ENDPOINT_SEND {
            if let Some(worker) = &self.endpoint {
                match worker.send(text(view, ENDPOINT_REPLY)) {
                    Ok(()) => set_text(view, ENDPOINT_REPLY, ""),
                    Err(msg) => set_text(view, LOCAL_STATUS, &msg),
                }
            }
        } else if id == ENDPOINT_CLOSE && self.endpoint.take().is_some() {
            set_text(
                view,
                LOCAL_STATUS,
                "Endpoint session closed. Check the endpoint queue for saved work.",
            );
        } else if id == LOCAL_CANCEL && self.polling {
            self.cancel_requested = true;
            self.check = None;
            set_text(
                view,
                LOCAL_STATUS,
                "Requesting cancellation at the next inference checkpoint...",
            );
        } else if self.editable() {
            match id {
                LOCAL_INPUT_BROWSE => {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_title("Select an MP4 for local transcription")
                        .add_filter("MP4 video", &["mp4"])
                        .pick_file()
                    {
                        set_text(view, LOCAL_INPUT, &path.to_string_lossy());
                        self.changed(view);
                    }
                }
                LOCAL_OUTPUT_BROWSE | LOCAL_MODEL_BROWSE => {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_title(if id == LOCAL_MODEL_BROWSE {
                            "Select the extracted Whisper base.en model folder"
                        } else {
                            "Select an existing transcript folder"
                        })
                        .pick_folder()
                    {
                        set_text(
                            view,
                            if id == LOCAL_MODEL_BROWSE {
                                LOCAL_MODEL
                            } else {
                                LOCAL_OUTPUT
                            },
                            &path.to_string_lossy(),
                        );
                        self.changed(view);
                    }
                }
                LOCAL_IMPORT => {
                    let dir = text(view, LOCAL_MODEL).trim().to_owned();
                    if dir.is_empty() {
                        set_text(
                            view,
                            LOCAL_STATUS,
                            "Extract the base.en model pack. Choose its folder and select Import base.en pack.",
                        );
                    } else {
                        self.discard_plan();
                        set_text(
                            view,
                            LOCAL_STATUS,
                            "Verifying and importing the local base.en model pack…",
                        );
                        self.spawn(move || install(dir));
                    }
                }
                LOCAL_PREVIEW => self.start_preview(view),
                LOCAL_APPROVE => self.accept(view),
                LOCAL_PREVIOUS => self.fetch(self.offset.saturating_sub(PAGE)),
                LOCAL_NEXT => {
                    if let Some(next) = self.next {
                        self.fetch(next);
                    }
                }
                ENDPOINT_FILES | ENDPOINT_QUEUE => self.endpoint(view, id == ENDPOINT_FILES),
                _ => {}
            }
        }
        self.controls(view);
    }

    fn update(&mut self, view: &View, reply: Reply) -> Result<(), String> {
        match reply {
            Reply::Reopened(Some(job)) => {
                self.job = Some(job);
                self.polling = true;
                self.check = None;
                set_text(view, LOCAL_STATUS, "Opening the last local job…");
            }
            Reply::Reopened(None) => self.spawn(model_status),
            Reply::Model(installed) => {
                set_text(
                    view,
                    LOCAL_STATUS,
                    if installed {
                        "Local Whisper base.en is ready. Select an MP4 and preview the job."
                    } else {
                        "Extract the English base.en model pack. Select its folder below and choose Import base.en pack."
                    },
                );
            }
            Reply::Imported => set_text(
                view,
                LOCAL_STATUS,
                "Model installed. Local Whisper base.en is ready.",
            ),
            Reply::Preview(plan) => {
                set_text(view, LOCAL_STATUS, &plan.description);
                self.plan = Some(plan);
            }
            Reply::Accepted(job) => {
                self.plan = None; // Keep the grant for status, review and restart.
                self.job = Some(job.clone());
                self.polling = true;
                self.check = None;
                set_text(view, LOCAL_STATUS, "Local job accepted.");
            }
            Reply::Status(reply) => {
                let state = object_field(&reply, "state")?;
                let frames = reply["processed_frames"].as_u64().unwrap_or_default();
                let count = reply["segment_count"].as_u64().unwrap_or_default();
                self.polling =
                    !matches!(state, "completed" | "failed" | "cancelled" | "interrupted");
                if state == "completed" {
                    self.fetch(0);
                } else {
                    let error = reply["error"]
                        .as_str()
                        .or_else(|| reply["error"]["message"].as_str())
                        .unwrap_or("");
                    set_changed(
                        view,
                        LOCAL_STATUS,
                        &format!(
                            "Local job {state}. Processed {} of source audio. {count} segments. {error}",
                            clock(frames / 16_000)
                        ),
                    );
                }
            }
            Reply::Page(page, offset) => {
                self.offset = offset;
                self.next = page["next_offset"].as_u64();
                let content = format_page(&page)?;
                let end = offset + page["segments"].as_array().map_or(0, |s| s.len() as u64);
                let summary = if end == 0 {
                    "No speech segments.".into()
                } else {
                    format!("Showing segments {}–{end}.", offset + 1)
                };
                set_text(view, LOCAL_REVIEW, &content);
                set_text(
                    view,
                    LOCAL_STATUS,
                    &format!(
                        "Completed. {summary}\r\nText: {}\r\nTimed JSON: {}",
                        object_field(&page, "text_path")?,
                        object_field(&page, "json_path")?
                    ),
                );
            }
        }
        Ok(())
    }

    pub(super) fn poll(&mut self, view: &View) {
        if !self.needs_tick() {
            return;
        }
        if let Some(endpoint) = &mut self.endpoint {
            let mut changed = false;
            for part in endpoint.output.try_iter().take(32) {
                self.log.extend(part);
                changed = true;
            }
            if self.log.len() > 65536 {
                self.log.drain(..self.log.len() - 65536);
            }
            if changed {
                let output = String::from_utf8_lossy(&self.log)
                    .replace("\r\n", "\n")
                    .replace('\n', "\r\n");
                set_text(view, LOCAL_REVIEW, &output);
                msg(view, LOCAL_REVIEW, EM_LINESCROLL, 0, i32::MAX as isize);
            } else if let Ok(Some(success)) = endpoint.done() {
                self.endpoint = None;
                set_text(
                    view,
                    LOCAL_STATUS,
                    if success {
                        "Endpoint session finished."
                    } else {
                        "Endpoint worker reported an error. See the details below."
                    },
                );
            }
        }
        if let Some(rx) = &self.busy {
            let reply = match rx.try_recv() {
                Ok(reply) => Some(reply),
                Err(TryRecvError::Disconnected) => Some(Err(
                    "The worker connection closed. Reopen the current job.".into(),
                )),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(reply) = reply {
                self.busy = None;
                self.approving = false;
                if let Err(e) = reply.and_then(|r| self.update(view, r)) {
                    self.polling = false;
                    self.cancel_requested = false;
                    set_text(view, LOCAL_STATUS, &e);
                }
            }
        }
        if self.visible
            && self.polling
            && self.busy.is_none()
            && (self.cancel_requested || self.check.is_none_or(|t| t.elapsed() >= STATUS_PERIOD))
        {
            if let Some(job) = self.job.clone() {
                self.check = Some(Instant::now());
                let action = if std::mem::take(&mut self.cancel_requested) {
                    "cancel"
                } else {
                    "status"
                };
                self.spawn(move || {
                    request(
                        "batch",
                        action,
                        Some(&job.grant),
                        json!({"batch_id":job.batch}),
                    )
                    .map(Reply::Status)
                });
            }
        } else if !self.polling {
            self.cancel_requested = false;
        }
        self.controls(view);
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        self.discard_plan();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_review_keeps_source_times_without_starting_playback() {
        let page = json!({"segments":[{"start_ms":30567,"end_ms":32412,"text":" A test. "}]});
        assert_eq!(
            format_page(&page).unwrap(),
            "[00:00:30.567 - 00:00:32.412] A test."
        );
        assert!(format_page(&json!({"segments":[{"text":"untimed"}]})).is_err());
    }
}
