use super::alignment::Assignment;
use super::storage::{open_owned, write_json};
use super::{CHECKPOINT, JOURNAL, LiveCache, PARTIAL, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};

// The transport already caps entire responses. This independent record limit
// bounds the amount that a torn-checkpoint recovery can read into memory.
const MAX_RECORD: usize = 4 * 1024 * 1024;
const MAX_SOURCES: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct Row {
    pub id: String,
    pub source: String,
    pub start_frame: u64,
    pub end_frame: u64,
}

impl Row {
    pub fn new(id: &str, source: &str, start: u64, end: u64) -> Result<Self> {
        let row = Self {
            id: id.into(),
            source: source.into(),
            start_frame: start,
            end_frame: end,
        };
        row.check()?;
        Ok(row)
    }
    fn check(&self) -> Result<()> {
        if self.id.is_empty()
            || self.id.len() > 256
            || self.source.is_empty()
            || self.source.len() > 1024
            || self.end_frame < self.start_frame
        {
            return Err("Invalid live segment identity, source or frame range".into());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TimedSpeech {
    pub start_frame: u64,
    pub end_frame: u64,
    pub speaker: Option<String>,
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum Event {
    Begin {
        schema: u32,
        session: String,
        interval: String,
        start_frame: u64,
        #[serde(default)]
        sources: Vec<String>,
    },
    Request(Row),
    Speech {
        #[serde(flatten)]
        row: Row,
        speaker: Option<String>,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        segments: Vec<TimedSpeech>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        assignments: Vec<Assignment>,
    },
    Gap {
        #[serde(flatten)]
        row: Row,
        reason: String,
    },
    Skipped(Row),
    Recording {
        id: String,
        start_frame: u64,
        end_frame: u64,
    },
    Timing {
        source: String,
        frame: u64,
    },
    SpeakerRevision {
        source: String,
        start_frame: u64,
        end_frame: u64,
        revision: u64,
        update: serde_json::Value,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        assignments: Vec<Assignment>,
    },
    End {
        cutoff_frame: u64,
        incomplete: bool,
        speech_entries: u64,
        gaps: u64,
    },
}

#[derive(Serialize, Deserialize)]
struct Entry {
    sequence: u64,
    #[serde(flatten)]
    event: Event,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Checkpoint {
    sequence: u64,
    // Previous offsets bound the validation of the last committed row.
    previous_journal: u64,
    previous_text: u64,
    journal: u64,
    text: u64,
    pub(super) last_frame: u64,
    pub(super) pending: BTreeMap<String, Row>,
    cursors: BTreeMap<String, Row>,
    pub(super) speech: u64,
    pub(super) gaps: u64,
    pub(super) sealed: bool,
}

fn stamp(frame: u64) -> String {
    let ms = frame / 48;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

fn label(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

impl Event {
    fn row(&self) -> Option<&Row> {
        match self {
            Self::Request(row)
            | Self::Speech { row, .. }
            | Self::Gap { row, .. }
            | Self::Skipped(row) => Some(row),
            _ => None,
        }
    }

    fn render(&self) -> String {
        match self {
            Self::Begin {
                session, interval, ..
            } => format!("Audio Capture | session {session} | transcript {interval}\n"),
            Self::Speech {
                row,
                segments,
                assignments,
                ..
            } if !segments.is_empty() => {
                let mut out = String::new();
                let mut local = assignments.iter().peekable();
                for (ordinal, segment) in segments.iter().enumerate() {
                    let inferred = if local.peek().is_some_and(|a| a.ordinal == ordinal) {
                        local.next()
                    } else {
                        None
                    };
                    let voice = if let Some((a, s)) =
                        inferred.and_then(|a| a.speaker.as_ref().map(|s| (a, s)))
                    {
                        format!(
                            " | provisional speaker {} (revision {}, span {}/{})",
                            label(s),
                            a.revision,
                            label(&row.id),
                            ordinal
                        )
                    } else {
                        segment
                            .speaker
                            .as_deref()
                            .map(|s| format!(" | provider speaker {}", label(s)))
                            .unwrap_or_default()
                    };
                    out.push_str(&format!(
                        "[Speech] {} | timed {} - {}{}\n",
                        label(&row.source),
                        stamp(segment.start_frame),
                        stamp(segment.end_frame),
                        voice
                    ));
                    for line in segment.text.lines() {
                        out.push_str("  ");
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                out
            }
            Self::Speech {
                row, speaker, text, ..
            } if !text.trim().is_empty() => {
                let voice = speaker
                    .as_deref()
                    .map(|s| format!(" | speaker {}", label(s)))
                    .unwrap_or_default();
                let mut out = format!(
                    "[Speech] {} | chunk {} - {}{}\n",
                    label(&row.source),
                    stamp(row.start_frame),
                    stamp(row.end_frame),
                    voice
                );
                for line in text.lines() {
                    out.push_str("  ");
                    out.push_str(line);
                    out.push('\n');
                }
                out
            }
            Self::Gap { row, reason } => format!(
                "[Gap] {} | {} - {} | {}\n",
                label(&row.source),
                stamp(row.start_frame),
                stamp(row.end_frame),
                label(reason)
            ),
            Self::Timing { source, frame } => format!(
                "[Timing] {} | {} | source timing uncertain\n",
                label(source),
                stamp(*frame)
            ),
            Self::SpeakerRevision {
                source,
                start_frame,
                end_frame,
                revision,
                assignments,
                update,
            } => {
                let mut text = format!(
                    "[Speaker update] {} | {} - {} | revision {}\n",
                    label(source),
                    stamp(*start_frame),
                    stamp(*end_frame),
                    revision
                );
                for a in assignments {
                    let who = a
                        .speaker
                        .as_deref()
                        .map(|s| format!("provisional {}", label(s)))
                        .unwrap_or_else(|| "unassigned".into());
                    text.push_str(&format!(
                        "  span {}/{} | {} - {} | {}\n",
                        label(&a.id),
                        a.ordinal,
                        stamp(a.start_frame),
                        stamp(a.end_frame),
                        who
                    ));
                }
                if assignments.is_empty() && update["turns"].as_array().is_some_and(Vec::is_empty) {
                    text.push_str("  No speaker assigned in this revision.\n");
                }
                text
            }
            Self::End {
                incomplete,
                speech_entries,
                ..
            } => {
                let status = if *incomplete {
                    "Incomplete coverage"
                } else if *speech_entries == 0 {
                    "No speech recognized"
                } else {
                    "Complete"
                };
                format!("[Status] {status}\n")
            }
            _ => String::new(),
        }
    }
}

impl Checkpoint {
    pub(super) fn published() -> Self {
        Self {
            sealed: true,
            ..Self::default()
        }
    }
    /// Returns false for an already committed delivery. Each source owns its
    /// frame ranges in order; a cursor suffices for arbitrarily long calls.
    fn accept(&self, event: &Event, meta: &super::Interval) -> Result<bool> {
        let origin = meta.start;
        if self.sealed {
            return Err("Live interval already sealed".into());
        }
        let source = match event {
            Event::Timing { source, .. } | Event::SpeakerRevision { source, .. } => Some(source),
            _ => event.row().map(|row| &row.source),
        };
        if source.is_some_and(|s| !meta.sources.contains(s)) {
            return Err("Live event contains an unapproved source".into());
        }
        if let Some(row) = event.row() {
            row.check()?;
            if row.start_frame < origin {
                return Err("Live segment precedes the interval".into());
            }
            if let Some(last) = self.cursors.get(&row.source) {
                if row.end_frame <= last.end_frame && row.start_frame < last.end_frame {
                    if matches!(event, Event::Request(_)) {
                        return Err("Recognition range already committed".into());
                    }
                    if row.end_frame == last.end_frame && row.id != last.id {
                        return Err("Conflicting live segment at the source cursor".into());
                    }
                    return Ok(false);
                }
                if row.start_frame < last.end_frame {
                    return Err("Live source ranges overlap".into());
                }
                if last == row {
                    return Ok(false);
                }
            } else if self.cursors.len() >= MAX_SOURCES {
                return Err("Too many live transcript sources".into());
            }
            if let Some(request) = self.pending.get(&row.source) {
                if matches!(event, Event::Request(_)) {
                    return Err("A request already has an unknown outcome for this source".into());
                }
                if request != row {
                    return Err("Live response does not match its pending request".into());
                }
            }
            if matches!(event, Event::Request(_)) && self.pending.len() >= MAX_SOURCES {
                return Err("Too many outstanding live requests".into());
            }
        }
        match event {
            Event::Begin {
                schema,
                start_frame,
                ..
            } if self.sequence != 0 || *schema != 1 || *start_frame != origin => {
                return Err("Invalid live journal header".into());
            }
            Event::Recording {
                start_frame,
                end_frame,
                ..
            }
            | Event::SpeakerRevision {
                start_frame,
                end_frame,
                ..
            } if *start_frame < origin || end_frame < start_frame => {
                return Err("Invalid annotation bounds".into());
            }
            Event::Timing { frame, .. } if *frame < origin => {
                return Err("Timing marker precedes interval".into());
            }
            Event::End { cutoff_frame, .. }
                if *cutoff_frame < self.last_frame || *cutoff_frame < origin =>
            {
                return Err("Live cutoff precedes committed data".into());
            }
            _ => {}
        }
        Ok(true)
    }

    fn advance(&mut self, event: &Event, journal_len: u64, text_len: u64) -> Result<()> {
        self.previous_journal = self.journal;
        self.previous_text = self.text;
        self.journal = self
            .journal
            .checked_add(journal_len)
            .ok_or("Journal length overflow")?;
        self.text = self
            .text
            .checked_add(text_len)
            .ok_or("Text length overflow")?;
        self.sequence += 1;
        if let Some(row) = event.row() {
            self.last_frame = self.last_frame.max(row.end_frame);
            if matches!(event, Event::Request(_)) {
                self.pending.insert(row.source.clone(), row.clone());
            } else {
                self.pending.remove(&row.source);
                self.cursors.insert(row.source.clone(), row.clone());
            }
        }
        match event {
            Event::Speech { text, .. } if !text.trim().is_empty() => self.speech += 1,
            Event::Gap { .. } => self.gaps += 1,
            Event::Timing { frame, .. } => self.last_frame = self.last_frame.max(*frame),
            Event::Recording { end_frame, .. } | Event::SpeakerRevision { end_frame, .. } => {
                self.last_frame = self.last_frame.max(*end_frame);
            }
            Event::End { .. } => self.sealed = true,
            _ => {}
        }
        Ok(())
    }
}

fn at_end(file: &mut File, expected: u64) -> Result<()> {
    if file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())? != expected {
        return Err("Unexpected live cache length; reopen recovery".into());
    }
    Ok(())
}
fn write_durable(file: &mut File, bytes: &[u8]) -> Result<()> {
    if !bytes.is_empty() {
        file.write_all(bytes)
            .and_then(|()| file.sync_data())
            .map_err(|e| e.to_string())?
    }
    Ok(())
}

impl LiveCache {
    pub(super) fn append(&mut self, mut event: Event) -> Result<()> {
        if self.poisoned || self.lock.is_none() {
            return Err("Live cache requires recovery".into());
        }
        if !self.state.accept(&event, &self.meta)? {
            return Ok(());
        }
        self.alignment.enrich(&mut event);
        let entry = Entry {
            sequence: self
                .state
                .sequence
                .checked_add(1)
                .ok_or("Live sequence limit reached")?,
            event,
        };
        let mut bytes = serde_json::to_vec(&entry).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        if bytes.len() > MAX_RECORD {
            return Err("Live record exceeds 4 MiB".into());
        }
        self.poisoned = true;
        let mut journal = open_owned(&self.owner.path.join(JOURNAL), true)?;
        at_end(&mut journal, self.state.journal)?;
        write_durable(&mut journal, &bytes)?;
        let text = entry.event.render();
        let mut partial = open_owned(&self.partial_path(), true)?;
        at_end(&mut partial, self.state.text)?;
        write_durable(&mut partial, text.as_bytes())?;
        self.state
            .advance(&entry.event, bytes.len() as u64, text.len() as u64)?;
        write_json(&self.owner.path, CHECKPOINT, &self.state)?;
        self.poisoned = false;
        if matches!(
            entry.event,
            Event::Speech { .. } | Event::SpeakerRevision { .. }
        ) {
            self.preview = if text.is_empty() {
                None
            } else {
                Some(
                    text.trim()
                        .chars()
                        .take(512)
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .collect(),
                )
            };
        }
        Ok(())
    }

    pub(super) fn replay_tail(&mut self) -> Result<()> {
        let mut journal = open_owned(&self.owner.path.join(JOURNAL), true)?;
        let mut partial = open_owned(&self.owner.path.join(PARTIAL), true)?;
        let length = journal.metadata().map_err(|e| e.to_string())?.len();
        let text_length = partial.metadata().map_err(|e| e.to_string())?.len();
        if self.state.journal > length
            || self.state.text > text_length
            || self.state.previous_journal > self.state.journal
            || self.state.previous_text > self.state.text
            || self.state.journal - self.state.previous_journal > MAX_RECORD as u64
            || length - self.state.journal > MAX_RECORD as u64
            || self.state.pending.len() > MAX_SOURCES
            || self.state.cursors.len() > MAX_SOURCES
        {
            return Err("Live files disagree with their durable checkpoint".into());
        }
        // The previous successful commit was fully flushed before its checkpoint.
        // Validate its exact text and sequence before modifying a recovery file.
        if self.state.sequence > 0 {
            journal
                .seek(SeekFrom::Start(self.state.previous_journal))
                .map_err(|e| e.to_string())?;
            let mut data = vec![0; (self.state.journal - self.state.previous_journal) as usize];
            journal.read_exact(&mut data).map_err(|e| e.to_string())?;
            let last: Entry = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
            let text = last.event.render();
            if data.last() != Some(&b'\n')
                || last.sequence != self.state.sequence
                || text.len() as u64 != self.state.text - self.state.previous_text
            {
                return Err("Invalid last live journal checkpoint".into());
            }
            partial
                .seek(SeekFrom::Start(self.state.previous_text))
                .map_err(|e| e.to_string())?;
            let mut verify = vec![0; text.len()];
            partial.read_exact(&mut verify).map_err(|e| e.to_string())?;
            if verify != text.as_bytes() {
                return Err("Committed live text was changed".into());
            }
        }
        partial
            .set_len(self.state.text)
            .map_err(|e| e.to_string())?;
        journal
            .seek(SeekFrom::Start(self.state.journal))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        BufReader::new(&journal)
            .take(MAX_RECORD as u64 + 1)
            .read_until(b'\n', &mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.last() == Some(&b'\n') {
            let entry: Entry = serde_json::from_slice(&bytes)
                .map_err(|e| format!("Invalid recovery record: {e}"))?;
            if entry.sequence
                != self
                    .state
                    .sequence
                    .checked_add(1)
                    .ok_or("Live sequence limit reached")?
                || !self.state.accept(&entry.event, &self.meta)?
            {
                return Err("Unexpected live recovery sequence".into());
            }
            let text = entry.event.render();
            at_end(&mut partial, self.state.text)?;
            write_durable(&mut partial, text.as_bytes())?;
            self.state
                .advance(&entry.event, bytes.len() as u64, text.len() as u64)?;
            write_json(&self.owner.path, CHECKPOINT, &self.state)?;
        }
        journal
            .set_len(self.state.journal)
            .and_then(|()| journal.sync_data())
            .map_err(|e| e.to_string())?;
        if self.state.sequence == 0 {
            self.append(Event::Begin {
                schema: super::SCHEMA,
                session: self.meta.session.clone(),
                interval: self.meta.interval.clone(),
                start_frame: self.meta.start,
                sources: self.meta.sources.clone(),
            })?;
        }
        Ok(())
    }
}
