//! Connect the live controls to the shared capture session and separate worker.
use super::*;
use audio_capture::live_wire::{QUALIFIED_LIVE_GATE, QUALIFIED_LIVE_SPEAKERS};
use audio_capture::transcription_config::{FILENAME, TranscriptionConfig, validate_connection};
use std::path::Path;

fn config_from_controls(view: &View) -> TranscriptionConfig {
    TranscriptionConfig {
        endpoint: text(view, LIVE_ENDPOINT).trim().to_owned(),
        model: text(view, LIVE_MODEL).trim().to_owned(),
        api_key_env: text(view, LIVE_KEY_ENV).trim().to_owned(),
        transcript_output_folder: PathBuf::from(text(view, LIVE_OUTPUT_DIR).trim()),
        request_segment_timestamps: checked(view, LIVE_TIMESTAMPS),
        speech_gating: QUALIFIED_LIVE_GATE && checked(view, LIVE_GATE),
        speaker_labels: QUALIFIED_LIVE_SPEAKERS && checked(view, LIVE_SPEAKERS),
        speaker_adapter_config: PathBuf::from(text(view, LIVE_SPEAKER_CONFIG).trim()),
        ..TranscriptionConfig::default()
    }
}

pub(super) fn batch_transcription_config(view: &View) -> Option<TranscriptionConfig> {
    let mut config = config_from_controls(view);
    if config.endpoint.is_empty() && config.model.is_empty() && config.api_key_env.is_empty() {
        return None;
    }
    if config.transcript_output_folder.as_os_str().is_empty() {
        config.transcript_output_folder = PathBuf::from(text(view, FOLDER));
    }
    Some(config)
}

fn apply_config(view: &View, config: &TranscriptionConfig) {
    for (id, value) in [
        (LIVE_ENDPOINT, config.endpoint.as_str()),
        (LIVE_MODEL, config.model.as_str()),
        (LIVE_KEY_ENV, config.api_key_env.as_str()),
        (
            LIVE_OUTPUT_DIR,
            config.transcript_output_folder.to_str().unwrap_or_default(),
        ),
        (
            LIVE_SPEAKER_CONFIG,
            config.speaker_adapter_config.to_str().unwrap_or_default(),
        ),
    ] {
        set_text(view, id, value);
    }
    set_checked(view, LIVE_TIMESTAMPS, config.request_segment_timestamps);
    set_checked(view, LIVE_GATE, config.speech_gating);
    set_checked(view, LIVE_SPEAKERS, config.speaker_labels);
}

fn save_config(view: &View, destination: &Path) -> Result<(), String> {
    let mut config = config_from_controls(view);
    for path in [
        &mut config.transcript_output_folder,
        &mut config.speaker_adapter_config,
    ] {
        if !path.as_os_str().is_empty() {
            *path = std::path::absolute(&*path).map_err(|error| {
                format!("Cannot resolve a transcription settings path. {error}")
            })?;
        }
    }
    config.save(destination)
}

fn open_transcript(view: &View, path: &std::path::Path, folder: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    if !path.is_absolute()
        || !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
    {
        return Err("The worker has not supplied a local text file.".into());
    }
    let target = if folder {
        path.parent()
            .ok_or("The transcript has no output folder.")?
    } else {
        path
    };
    let argument: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let code = unsafe {
        ShellExecuteW(
            view.hwnd,
            wide("open").as_ptr(),
            argument.as_ptr(),
            ptr::null(),
            ptr::null(),
            1,
        )
    } as isize;
    if code <= 32 {
        Err(format!(
            "Windows could not open the transcript location (code {code})."
        ))
    } else {
        Ok(())
    }
}

impl Controller {
    pub(super) fn transcription_connection_changed(&mut self, view: &View) {
        let endpoint = text(view, LIVE_ENDPOINT);
        let model = text(view, LIVE_MODEL);
        let key_env = text(view, LIVE_KEY_ENV);
        self.live_note = if endpoint.trim().is_empty() || model.trim().is_empty() {
            TRANSCRIPTION_SETUP_NEEDED.into()
        } else {
            match validate_connection(endpoint.trim(), model.trim(), key_env.trim()) {
                Ok(()) => "Transcription settings configured.".into(),
                Err(error) => format!("Check Transcription settings. {error}"),
            }
        };
        self.show_live(view);
    }

    pub(crate) fn load_startup_transcription_config(&mut self, view: &View, exe: &Path) {
        match TranscriptionConfig::load_optional_next_to(exe) {
            Ok(Some(config)) => {
                apply_config(view, &config);
                self.live_note = format!("Loaded transcription config from {FILENAME}");
            }
            Ok(None) => {}
            Err(error) => {
                self.live_note = format!("Could not load {FILENAME}. {error}");
                view.show_transcription_settings();
            }
        }
    }

    pub(super) fn transcription_config_command(&mut self, view: &View, id: i32) {
        let loading = id == LOAD_LIVE_CONFIG;
        let title = if loading {
            "Load transcription config"
        } else {
            "Save transcription config"
        };
        let mut dialog = rfd::FileDialog::new()
            .set_title(title)
            .add_filter("Transcription config", &["json"]);
        if let Some(folder) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        {
            dialog = dialog.set_directory(folder);
        }
        let path = if loading {
            dialog.pick_file()
        } else {
            dialog.set_file_name(FILENAME).save_file()
        };
        let Some(path) = path else { return };
        let result = if loading {
            TranscriptionConfig::load(&path).map(|config| apply_config(view, &config))
        } else {
            save_config(view, &path)
        };
        let file = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        self.live_note = match result {
            Ok(()) if loading => format!("Loaded transcription config from {file}"),
            Ok(()) => format!("Saved transcription config to {file}"),
            Err(error) => {
                unsafe {
                    MessageBoxW(
                        view.hwnd,
                        wide(&error).as_ptr(),
                        wide(title).as_ptr(),
                        MB_OK | MB_ICONERROR,
                    );
                }
                error
            }
        };
    }

    fn displayed_live(&self) -> LiveStatus {
        self.live
            .as_ref()
            .or_else(|| self.finishing.last())
            .map_or_else(|| self.last_live.clone(), LiveWorker::status)
    }

    pub(super) fn displayed_path(&self) -> Option<PathBuf> {
        self.live
            .as_ref()
            .and_then(LiveWorker::path)
            .or_else(|| self.finishing.iter().rev().find_map(LiveWorker::path))
            .or_else(|| self.last_live.path.clone())
    }

    pub(super) fn show_live(&self, view: &View) {
        let status = self.displayed_live();
        let mut text = if let Some(worker) = &self.live {
            let state = if worker.started.is_some()
                && matches!(status.state.as_str(), "ready" | "waiting")
            {
                "Starting confirmed sources".to_owned()
            } else if let Some(started) = worker.started {
                format!("Live {}", clock(started.elapsed().as_secs()))
            } else {
                "Waiting for transcription worker".to_owned()
            };
            let mut text = format!(
                "{state}  |  {}  |  {}",
                worker.options.sources.join(" · "),
                worker.options.endpoint
            );
            if !self.finishing.is_empty() {
                text.push_str(&format!(
                    "  |  Saving {} earlier transcript(s)",
                    self.finishing.len()
                ));
            }
            text
        } else if !self.finishing.is_empty() {
            format!("Saving transcript ({})", self.finishing.len())
        } else {
            self.live_note.clone()
        };
        if !status.message.is_empty() && !text.contains(&status.message) {
            text.push_str("  |  ");
            text.push_str(&status.message);
        }
        if let Some(path) = status.path {
            text.push_str("  |  ");
            text.push_str(&path.to_string_lossy());
        } else if let Some(path) = self.displayed_path() {
            text.push_str("  |  Previous transcript: ");
            text.push_str(&path.to_string_lossy());
        }
        set_changed(view, LIVE_STATUS, &text);
        set_changed(
            view,
            LIVE_PREVIEW,
            &status.preview.replace("\r\n", "\n").replace('\n', "\r\n"),
        );
    }

    fn start_live(&mut self, view: &View) -> Result<(), String> {
        if self.finishing.len() >= 4 || (self.session.is_none() && self.retiring.len() >= 4) {
            return Err("Wait for a pending transcript to finish before starting another.".into());
        }
        self.read_controls(view);
        if !self.confirm_source_selection(view) {
            return Ok(());
        }
        let config = self.config()?;
        if config.monitor_only {
            return Err("Turn off Monitor only before starting transcription.".into());
        }
        let sources: Vec<String> = config.inputs.iter().map(|(s, _, _)| s.clone()).collect();
        if sources.len() > 16 {
            return Err("Live transcription supports up to 16 selected sources.".into());
        }
        let transcription = config_from_controls(view);
        let folder = if transcription
            .transcript_output_folder
            .as_os_str()
            .is_empty()
        {
            self.settings.folder.clone()
        } else {
            transcription.transcript_output_folder
        };
        let options = LiveOptions {
            endpoint: transcription.endpoint,
            model: transcription.model,
            key_env: transcription.api_key_env,
            folder: std::path::absolute(folder)
                .map_err(|e| format!("Cannot resolve the transcript folder: {e}"))?,
            sources,
            gate: transcription.speech_gating,
            timestamps: transcription.request_segment_timestamps,
            speaker_config: (QUALIFIED_LIVE_SPEAKERS
                && !transcription.speaker_adapter_config.as_os_str().is_empty())
            .then_some(transcription.speaker_adapter_config),
        };
        if let Err(error) = options.validate() {
            view.show_transcription_settings();
            return Err(error);
        }
        let speakers = transcription.speaker_labels;
        if speakers && options.speaker_config.is_none() {
            view.show_transcription_settings();
            return Err("Choose a qualified local speaker configuration first.".into());
        }
        let question = options.confirmation(&self.scope_summary(), speakers);
        let yes = unsafe {
            MessageBoxW(
                view.hwnd,
                wide(&question).as_ptr(),
                wide("Confirm live transcription").as_ptr(),
                0x124,
            )
        } == 6;
        if !yes {
            self.live_note = "Cancelled. No audio was sent.".into();
            return Ok(());
        }
        let worker = LiveWorker::spawn(options)?;
        self.live = Some(worker);
        self.pending_config = Some(config);
        self.live_note = "Starting transcription".into();
        Ok(())
    }

    fn admit_live(&mut self, view: &View) -> Result<(), String> {
        let config = self
            .pending_config
            .take()
            .ok_or("Missing approved source scope.")?;
        // Recheck the original process instance after a potentially long wait for a batch request.
        for (_, input, _) in &config.inputs {
            if let capture::Input::SelectedProcess { pid, created } = input {
                if sources::process_identity(*pid).ok() != Some(*created) {
                    return Err("The approved program exited. Refresh and select it again.".into());
                }
            }
        }
        let worker = self
            .live
            .as_mut()
            .ok_or("Transcription start was cancelled.")?;
        let tap = worker.tap()?;
        if let Some(session) = &self.session {
            session.attach_live(tap)?;
        } else {
            self.session = Some(record::Session::start_live(config, tap)?);
        }
        worker.started = Some(Instant::now());
        if audio_capture::live_wire::QUALIFIED_LIVE_SPEAKERS && checked(view, LIVE_SPEAKERS) {
            if let Some(session) = &self.session {
                if let Err(error) = session.set_live_speakers(true) {
                    set_checked(view, LIVE_SPEAKERS, false);
                    self.live_note = format!("Speaker labels unavailable: {error}");
                }
            }
        }
        self.remember_selection();
        Ok(())
    }

    fn stop_live(&mut self) -> Result<(), String> {
        let Some(worker) = self.live.as_ref() else {
            return Ok(());
        };
        if worker.started.is_some()
            && let Some(s) = &self.session
            && let Err(error) = s.stop_live()
            && s.transcribing()
        {
            return Err(error);
        }
        let mut worker = self.live.take().expect("live worker");
        self.pending_config = None;
        if worker.started.is_some() {
            self.finishing.push(worker);
            self.retire_if_stopped();
        } else {
            worker.cancel_wait();
            self.live_note = "Transcription start cancelled.".into();
        }
        Ok(())
    }

    pub(super) fn stop_both(&mut self) {
        let recording = self.recording();
        if let Some(session) = &self.session {
            if let Err(error) = session.stop_both() {
                self.status = error;
                if !session.is_finished()
                    && (session.recording() || session.transcribing() || self.settings.monitor_only)
                {
                    return;
                }
            }
        }
        self.retire_if_stopped();
        if recording {
            self.status = format!("Finishing recording  |  {}", self.settings.folder.display());
        }
        self.recording_started = None;
        self.paused = false;
        self.pending_config = None;
        if let Some(mut worker) = self.live.take() {
            if worker.started.is_some() {
                self.finishing.push(worker);
            } else {
                worker.cancel_wait();
                self.live_note = "Transcription start cancelled.".into();
            }
        }
    }

    pub(super) fn live_command(&mut self, view: &View, id: i32) {
        let result = match id {
            LIVE_TOGGLE if self.live.is_none() => self.start_live(view),
            LIVE_TOGGLE => self.stop_live(),
            STOP_BOTH => {
                self.stop_both();
                Ok(())
            }
            LIVE_CANCEL_PENDING => self
                .finishing
                .last()
                .map_or(Ok(()), LiveWorker::cancel_drain),
            LIVE_SPEAKERS if !audio_capture::live_wire::QUALIFIED_LIVE_SPEAKERS => {
                set_checked(view, LIVE_SPEAKERS, false);
                Err("Live speaker labels are available only for evaluation. Use offline attribution on a saved recording.".into())
            }
            LIVE_SPEAKERS if self.live.as_ref().is_some_and(|w| w.started.is_some()) => {
                let on = checked(view, LIVE_SPEAKERS);
                if on
                    && self
                        .live
                        .as_ref()
                        .is_none_or(|w| w.options.speaker_config.is_none())
                {
                    set_checked(view, LIVE_SPEAKERS, false);
                    Err("Start a new transcription interval with a local speaker configuration to enable labels.".into())
                } else {
                    self.session
                        .as_ref()
                        .map_or(Ok(()), |s| s.set_live_speakers(on))
                }
            }
            LIVE_OPEN_TEXT | LIVE_OPEN_FOLDER => self
                .displayed_path()
                .ok_or_else(|| "No readable transcript is available yet.".to_string())
                .and_then(|path| open_transcript(view, &path, id == LIVE_OPEN_FOLDER)),
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.live_note = error.clone();
            if self.live.is_some() || !self.finishing.is_empty() {
                unsafe {
                    MessageBoxW(
                        view.hwnd,
                        wide(&error).as_ptr(),
                        wide("Live transcription").as_ptr(),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
        }
    }

    pub(super) fn poll_live(&mut self, view: &View) {
        let ended = self.live.as_mut().and_then(LiveWorker::result);
        if let Some(result) = ended {
            let worker = self.live.take().expect("worker with completed status");
            if worker.started.is_some() {
                if let Some(s) = &self.session {
                    let _ = s.stop_live();
                }
            }
            self.pending_config = None;
            self.set_live_result(worker, result);
        } else if self
            .live
            .as_ref()
            .is_some_and(|w| w.started.is_none() && w.is_ready())
        {
            if let Err(error) = self.admit_live(view) {
                let _ = self.stop_live();
                self.live_note = error;
            }
        } else if self.live.as_ref().is_some_and(|w| w.started.is_some())
            && self.session.as_ref().is_none_or(|s| !s.transcribing())
        {
            self.finishing.push(self.live.take().expect("live worker"));
        }
        let mut index = 0;
        while index < self.finishing.len() {
            if let Some(result) = self.finishing[index].result() {
                let worker = self.finishing.remove(index);
                self.set_live_result(worker, result);
            } else {
                index += 1;
            }
        }
    }

    fn set_live_result(&mut self, worker: LiveWorker, result: Result<LiveStatus, String>) {
        match result {
            Ok(status) => self.transcript_completed(status, None),
            Err(error) => self.transcript_completed(worker.status(), Some(error)),
        }
    }

    pub(super) fn transcript_completed(&mut self, mut status: LiveStatus, error: Option<String>) {
        let failed = error.is_some() || status.state == "incomplete";
        self.live_note = if let Some(error) = error {
            status.message = error;
            "Transcription stopped"
        } else {
            match status.state.as_str() {
                "incomplete" => "Transcript incomplete. Open the text to review gaps.",
                "cancelled" => "Pending transcription cancelled",
                _ => "Transcript saved",
            }
        }
        .into();
        if self.closing && failed {
            let mut detail = format!("{}  |  {}", self.live_note, status.message);
            if let Some(path) = &status.path {
                detail.push_str(&format!("\nTranscript: {}", path.display()));
                self.close_recovery.get_or_insert_with(|| status.clone());
            }
            self.remember_close_error(&detail);
        }
        self.last_live = status;
    }
}
