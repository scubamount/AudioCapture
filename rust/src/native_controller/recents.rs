use super::*;
use audio_capture::recent_programs::{Discovery, History, Match, RecentProgram, Snapshot};
use std::path::Path;
use std::time::Duration;

const AUTO_REFRESH: Duration = Duration::from_secs(2);
const REFRESHING: &str = "Refreshing programs and audio devices.";

struct UsedSources {
    session: String,
    processes: Vec<(u32, u64)>,
    mic: Option<String>,
    system: bool,
    scheduled: bool,
}

struct VerifiedSelection {
    pid: u32,
    created: u64,
    program: RecentProgram,
    automatic: bool,
}

#[derive(Default)]
pub(super) struct Recents {
    history: History,
    discovery: Discovery,
    snapshot: Option<Snapshot>,
    recent_use: Option<RecentProgram>,
    save_needed: Option<UsedSources>,
    verified: Option<VerifiedSelection>,
    clear_when_idle: bool,
    last_refresh: Option<Instant>,
    manual_choice: bool,
    unreadable_history: bool,
}

impl Recents {
    pub(super) fn needs_tick(&self) -> bool {
        self.discovery.is_running()
            || self.recent_use.is_some()
            || self.save_needed.is_some()
            || self.clear_when_idle
            || (self.history.auto_select()
                && self.history.last_single().is_some()
                && !self.manual_choice)
    }

    pub(super) fn manual_source_choice(&mut self) {
        self.manual_choice = true;
        self.recent_use = None;
        self.verified = None;
        self.clear_when_idle = false;
    }
}

impl Controller {
    fn recent_scope_locked(&self) -> bool {
        self.session.is_some() || self.live.is_some() || self.closing
    }

    pub(super) fn initialize_recents(&mut self, view: &View) {
        match History::load() {
            Ok(history) => self.recents.history = history,
            Err(error) => {
                self.recents.unreadable_history = true;
                self.status = error;
            }
        }
        self.sync_recents(view);
        self.start_source_refresh();
        self.show_status(view);
    }

    pub(super) fn refresh_recent_sources(&mut self, view: &View) {
        self.status = REFRESHING.into();
        self.start_source_refresh();
        self.show_status(view);
    }

    fn start_source_refresh(&mut self) {
        let ready = self.ready_to_remember();
        let selected: Vec<_> = if ready {
            self.recents.save_needed.as_ref().unwrap().processes.clone()
        } else {
            self.selected
                .iter()
                .map(|(&pid, &time)| (pid, time))
                .collect()
        };
        if self
            .recents
            .discovery
            .refresh(&self.recents.history, &selected)
        {
            if ready {
                self.recents.save_needed.as_mut().unwrap().scheduled = true;
            }
            self.recents.last_refresh = Some(Instant::now());
        }
    }

    pub(super) fn remember_selection(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        if self.recents.unreadable_history || self.settings.monitor_only {
            return;
        }
        let mut processes: Vec<_> = self
            .selected
            .iter()
            .map(|(&pid, &time)| (pid, time))
            .collect();
        processes.sort_unstable();
        self.recents.save_needed = Some(UsedSources {
            session: session.session_id().into(),
            processes,
            mic: self.settings.mic.clone(),
            system: self.settings.system,
            scheduled: false,
        });
        if self.ready_to_remember() {
            self.start_source_refresh();
        }
    }

    fn ready_to_remember(&self) -> bool {
        self.recents.save_needed.as_ref().is_some_and(|used| {
            !used.scheduled
                && self
                    .remembered_session(&used.session)
                    .is_some_and(record::Session::has_activated)
        })
    }

    fn remembered_session(&self, id: &str) -> Option<&record::Session> {
        self.session
            .iter()
            .chain(&self.retiring)
            .find(|session| session.session_id() == id)
    }

    pub(super) fn recent_image(&self, pid: u32) -> Option<&Path> {
        self.recents.snapshot.as_ref()?.image(pid)
    }

    pub(super) fn recent_source_marker(&self) -> Option<PathBuf> {
        let chosen = self.recents.verified.as_ref()?;
        let last = self.recents.history.last_single()?;
        let program = self.recents.history.entries().get(last)?;
        if self.selected.len() != 1
            || self.selected.get(&chosen.pid) != Some(&chosen.created)
            || self.settings.system
            || chosen.program != *program
        {
            return None;
        }
        Some(program.image.clone())
    }

    fn save_recents(&mut self, history: History, view: &View) -> bool {
        if let Err(error) = history.save() {
            self.status = error;
            return false;
        }
        self.recents.history = history;
        self.sync_recents(view);
        true
    }

    fn sync_recents(&self, view: &View) {
        let index = selected(view, RECENT_PICKER);
        let entries = self.recents.history.entries();
        let items: Vec<_> = entries
            .iter()
            .map(|entry| format!("{}  |  {}", entry.name, entry.image.display()))
            .collect();
        if items.is_empty() {
            combo(view, RECENT_PICKER, &["No recent programs"], 0);
        } else {
            combo(view, RECENT_PICKER, &items, index.min(items.len() - 1));
        }
        set_checked(view, AUTO_SELECT_RECENT, self.recents.history.auto_select());
        self.recent_controls(view);
    }

    pub(super) fn recent_controls(&self, view: &View) {
        let entries = !self.closing
            && !self.recents.history.entries().is_empty()
            && !self.recents.unreadable_history;
        enabled(view, RECENT_PICKER, entries);
        enabled(view, USE_RECENT, entries && !self.recent_scope_locked());
        enabled(view, FORGET_RECENT, entries);
        enabled(view, CLEAR_RECENT, entries);
        enabled(
            view,
            AUTO_SELECT_RECENT,
            !self.recent_scope_locked() && !self.recents.unreadable_history,
        );
    }

    fn clear_idle_preselection(&mut self, removed: Option<&RecentProgram>) {
        if let Some(settings) = self.recents.verified.as_ref()
            && settings.automatic
            && removed.is_none_or(|entry| entry == &settings.program)
        {
            if self.recent_scope_locked() {
                self.recents.clear_when_idle = true;
                return;
            }
            if self.selected.get(&settings.pid) == Some(&settings.created) {
                self.selected.remove(&settings.pid);
            }
            if self.settings.mic == settings.program.mic {
                self.settings.mic = None;
            }
            self.recents.verified = None;
            self.settings.remembered_program = None;
        }
        if let Some(program) = removed
            && self.settings.remembered_program.as_ref() == Some(&program.image)
            && !self.recent_scope_locked()
        {
            self.missing
                .retain(|name| !name.eq_ignore_ascii_case(&program.name));
            self.settings.remembered_program = None;
        }
    }

    pub(super) fn recent_command(&mut self, view: &View, id: i32, notification: u16) -> bool {
        if id == RECENT_PICKER {
            return true;
        }
        if ![AUTO_SELECT_RECENT, USE_RECENT, FORGET_RECENT, CLEAR_RECENT].contains(&id) {
            return false;
        }
        if notification != BN_CLICKED || self.recents.unreadable_history {
            return true;
        }
        let index = selected(view, RECENT_PICKER);
        let mut history = self.recents.history.clone();
        match id {
            AUTO_SELECT_RECENT if !self.recent_scope_locked() => {
                let automatic = checked(view, AUTO_SELECT_RECENT);
                history.set_auto_select(automatic);
                if self.save_recents(history, view) {
                    if automatic {
                        self.start_source_refresh();
                    }
                    self.status = if automatic {
                        "Automatic recent program selection is on."
                    } else {
                        "Automatic recent program selection is off."
                    }
                    .into();
                } else {
                    self.sync_recents(view);
                }
            }
            USE_RECENT if !self.recent_scope_locked() => {
                if let Some(program) = history.entries().get(index).cloned() {
                    self.recents.manual_source_choice();
                    self.recents.recent_use = Some(program);
                    self.status = "Checking the selected recent program.".into();
                    self.start_source_refresh();
                }
            }
            FORGET_RECENT => {
                let removed = history.entries().get(index).cloned();
                if history.forget(index) && self.save_recents(history, view) {
                    self.clear_idle_preselection(removed.as_ref());
                    if self.recents.recent_use == removed {
                        self.recents.recent_use = None;
                    }
                    self.recents.save_needed = None;
                    self.status = "Recent program removed.".into();
                    if !self.recent_scope_locked() {
                        self.sync_source_selection(view);
                    }
                }
            }
            CLEAR_RECENT => {
                let removed = history
                    .last_single()
                    .and_then(|index| history.entries().get(index))
                    .cloned();
                history.clear();
                if self.save_recents(history, view) {
                    self.clear_idle_preselection(removed.as_ref());
                    self.recents.recent_use = None;
                    self.recents.save_needed = None;
                    self.status = "Recent programs cleared.".into();
                    if !self.recent_scope_locked() {
                        self.sync_source_selection(view);
                    }
                }
            }
            _ => {}
        }
        self.show_status(view);
        true
    }

    fn apply_recent_match(
        &mut self,
        view: &View,
        program: RecentProgram,
        result: Match,
        automatic: bool,
    ) {
        match result {
            Match::Unique { pid, created } => {
                self.selected.clear();
                self.selected.insert(pid, created);
                self.missing.clear();
                self.settings.system = false;
                self.settings.mic = program.mic.clone();
                self.status = if program
                    .mic
                    .as_ref()
                    .is_some_and(|target| !self.inputs.iter().any(|(id, _)| id == target))
                {
                    format!(
                        "Selected {}. Choose an available microphone or turn microphone off.",
                        program.name
                    )
                } else {
                    format!("Selected {}.", program.name)
                };
                self.recents.verified = Some(VerifiedSelection {
                    pid,
                    created,
                    program,
                    automatic,
                });
                self.sync_source_selection(view);
            }
            Match::Absent if !automatic => {
                self.status = format!(
                    "{} is not running. Open it and refresh the program list.",
                    program.name
                );
            }
            Match::Review => {
                let previous_selection_count = self.selected.len();
                if !automatic {
                    self.selected.retain(|pid, _| {
                        self.processes
                            .iter()
                            .find(|item| item.pid == *pid)
                            .is_none_or(|item| !item.name.eq_ignore_ascii_case(&program.name))
                    });
                }
                let new_review = !self.missing.contains(&program.name);
                if new_review {
                    self.missing.push(program.name.clone());
                }
                if new_review || self.selected.len() != previous_selection_count {
                    self.process_list(view);
                }
                self.status = format!(
                    "Select {} from the program list to confirm its identity.",
                    program.name
                );
            }
            Match::Absent => {}
        }
    }

    fn sync_source_selection(&mut self, view: &View) {
        self.process_list(view);
        set_checked(view, SYSTEM, self.settings.system);
        Self::device_combo(view, MIC, &self.inputs, &self.settings.mic);
    }

    fn next_automatic_program(&self) -> Option<RecentProgram> {
        if self.recent_scope_locked()
            || self.recents.manual_choice
            || !self.selected.is_empty()
            || self.settings.system
            || self.settings.monitor_only
        {
            return None;
        }
        self.recents
            .history
            .automatic_candidate(&self.missing, self.settings.remembered_program.as_deref())
            .cloned()
    }

    pub(super) fn poll_recent_sources(&mut self, view: &View) -> bool {
        let cleared = self.recents.clear_when_idle && !self.recent_scope_locked();
        if cleared {
            self.recents.clear_when_idle = false;
            self.clear_idle_preselection(None);
            self.sync_source_selection(view);
        }
        if self.recents.save_needed.as_ref().is_some_and(|used| {
            !used.scheduled
                && self
                    .remembered_session(&used.session)
                    .is_none_or(|session| session.is_finished() && !session.has_activated())
        }) {
            self.recents.save_needed = None;
        }
        let refresh_due = self
            .recents
            .last_refresh
            .is_none_or(|last| last.elapsed() >= AUTO_REFRESH);
        let Some(scan) = self.recents.discovery.poll() else {
            let watch = !self.recent_scope_locked()
                && self.recents.history.auto_select()
                && !self.recents.manual_choice
                && self.recents.verified.is_some();
            if self.ready_to_remember()
                || (refresh_due && (watch || self.next_automatic_program().is_some()))
            {
                self.start_source_refresh();
            }
            return cleared;
        };
        let mut snapshot = match scan {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.status = error;
                self.recents.recent_use = None;
                self.recents.save_needed = None;
                return true;
            }
        };
        if self.status == REFRESHING {
            self.status.clear();
        }
        self.record_recent_use(view, &snapshot);
        if self.recent_scope_locked() {
            return true;
        }
        if let Some(warning) = snapshot.device_error.take() {
            self.status = warning;
        } else {
            if self.inputs != snapshot.inputs {
                self.inputs = std::mem::take(&mut snapshot.inputs);
                Self::device_combo(view, MIC, &self.inputs, &self.settings.mic);
            }
            if self.outputs != snapshot.outputs {
                self.outputs = std::mem::take(&mut snapshot.outputs);
                Self::device_combo(view, MONITOR, &self.outputs, &self.settings.monitor);
            }
        }
        let visibility_changed = self.discovery_warning != snapshot.warning;
        self.discovery_warning = snapshot.warning.take();
        let current = std::mem::take(&mut snapshot.processes);
        let expired =
            retain_live_selections(&mut self.selected, &self.processes, &current, |pid| {
                snapshot
                    .created(pid)
                    .map(Ok)
                    .unwrap_or_else(|| sources::process_identity(pid))
            });
        if !expired.is_empty() {
            self.status = format!("Refresh the source choice for {}.", expired.join(", "));
        }
        let changed_automatic = self.recents.verified.as_ref().is_some_and(|entry| {
            entry.automatic
                && snapshot.resolve(&entry.program)
                    != (Match::Unique {
                        pid: entry.pid,
                        created: entry.created,
                    })
        });
        if changed_automatic {
            self.clear_idle_preselection(None);
            self.sync_source_selection(view);
        }
        let next = self
            .recents
            .recent_use
            .take()
            .map(|entry| (entry, false))
            .or_else(|| self.next_automatic_program().map(|entry| (entry, true)));
        let matched = next.map(|(program, automatic)| {
            let result = snapshot.resolve(&program);
            (program, result, automatic)
        });
        let changed = self.processes.len() != current.len()
            || self.processes.iter().zip(&current).any(|(old, new)| {
                old.pid != new.pid
                    || old.name != new.name
                    || old.active != new.active
                    || old.title != new.title
            })
            || !expired.is_empty()
            || visibility_changed
            || self.recents.snapshot.is_none();
        self.processes = current;
        self.recents.snapshot = Some(snapshot);
        if changed {
            self.process_list(view);
        }
        if let Some((program, result, automatic)) = matched {
            self.apply_recent_match(view, program, result, automatic);
        }
        true
    }

    fn record_recent_use(&mut self, view: &View, snapshot: &Snapshot) {
        let Some(used) = self.recents.save_needed.take() else {
            return;
        };
        if !used.scheduled {
            self.recents.save_needed = Some(used);
            if self.ready_to_remember() {
                self.start_source_refresh();
            }
            return;
        }
        let mut history = self.recents.history.clone();
        history.remember(snapshot, &used.processes, used.mic.as_deref(), used.system);
        if self.save_recents(history, view)
            && let ([(pid, created)], Some(index)) = (
                used.processes.as_slice(),
                self.recents.history.last_single(),
            )
            && self.selected.len() == 1
            && self.selected.get(pid) == Some(created)
        {
            let program = self.recents.history.entries()[index].clone();
            self.recents.manual_choice = false;
            self.recents.verified = Some(VerifiedSelection {
                pid: *pid,
                created: *created,
                program,
                automatic: self
                    .recents
                    .verified
                    .as_ref()
                    .is_some_and(|current| current.automatic),
            });
        }
    }
}
