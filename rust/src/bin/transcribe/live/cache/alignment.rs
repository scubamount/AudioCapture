//! Bounded alignment of local speaker evidence with immutable ASR spans.
//! Recovery replays the persisted assignments, never a model inference.
use super::journal::{Event, TimedSpeech};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

const MAX_SPANS: usize = 256;
const MAX_WINDOWS: usize = 12;
const HORIZON: u64 = 60 * 48_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Assignment {
    pub id: String,
    pub ordinal: usize,
    pub start_frame: u64,
    pub end_frame: u64,
    pub epoch: u64,
    pub revision: u64,
    pub evidence_start: u64,
    pub evidence_end: u64,
    pub speaker: Option<String>,
}

struct Span {
    id: String,
    ordinal: usize,
    start: u64,
    end: u64,
    previous: Option<(u64, u64, Option<String>)>,
}

#[derive(Deserialize)]
struct Turn {
    start: u64,
    end: u64,
    speaker: String,
}

struct Window {
    start: u64,
    end: u64,
    epoch: u64,
    revision: u64,
    turns: Vec<Turn>,
}
impl Window {
    fn speaker_for(&self, span: &Span) -> Option<&str> {
        if span.start < self.start || span.end > self.end || span.start == span.end {
            return None;
        }
        let mut candidates = self
            .turns
            .iter()
            .filter(|t| t.start < span.end && span.start < t.end);
        let first = candidates.next()?;
        // Competing voices or partial coverage are insufficient to assign a word.
        if first.start <= span.start
            && first.end >= span.end
            && candidates.all(|t| t.speaker == first.speaker)
        {
            Some(&first.speaker)
        } else {
            None
        }
    }
}

#[derive(Default)]
struct SourceMemory {
    spans: VecDeque<Span>,
    windows: VecDeque<Window>,
    newest: u64,
}

impl SourceMemory {
    fn trim(&mut self, frame: u64) {
        self.newest = self.newest.max(frame);
        let oldest = self.newest.saturating_sub(HORIZON);
        self.spans.retain(|s| s.end >= oldest);
        self.windows.retain(|w| w.end >= oldest);
        while self.spans.len() > MAX_SPANS {
            self.spans.pop_front();
        }
        while self.windows.len() > MAX_WINDOWS {
            self.windows.pop_front();
        }
    }
    fn assess(span: &mut Span, windows: &VecDeque<Window>) -> Option<Assignment> {
        let evidence = windows
            .iter()
            .filter(|w| w.start < span.end && span.start < w.end)
            .max_by_key(|w| (w.epoch, w.revision))?;
        let speaker = evidence.speaker_for(span).map(str::to_owned);
        let key = (evidence.epoch, evidence.revision, speaker.clone());
        if span.previous.as_ref() == Some(&key) {
            return None;
        }
        span.previous = Some(key);
        Some(Assignment {
            id: span.id.clone(),
            ordinal: span.ordinal,
            start_frame: span.start,
            end_frame: span.end,
            epoch: evidence.epoch,
            revision: evidence.revision,
            evidence_start: evidence.start,
            evidence_end: evidence.end,
            speaker,
        })
    }
    fn remember(
        &mut self,
        id: &str,
        end: u64,
        segments: &[TimedSpeech],
        assignments: &mut Vec<Assignment>,
    ) {
        self.trim(end);
        for (ordinal, s) in segments.iter().enumerate() {
            let mut span = Span {
                id: id.into(),
                ordinal,
                start: s.start_frame,
                end: s.end_frame,
                previous: None,
            };
            if let Some(decision) = Self::assess(&mut span, &self.windows) {
                assignments.push(decision);
            }
            if span.end >= self.newest.saturating_sub(HORIZON) {
                if self.spans.len() == MAX_SPANS {
                    self.spans.pop_front();
                }
                self.spans.push_back(span);
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Alignment {
    sources: BTreeMap<String, SourceMemory>,
}

impl Alignment {
    pub fn enrich(&mut self, event: &mut Event) {
        match event {
            Event::Speech {
                row,
                segments,
                assignments,
                ..
            } if !segments.is_empty() => self
                .sources
                .entry(row.source.clone())
                .or_default()
                .remember(&row.id, row.end_frame, segments, assignments),
            Event::SpeakerRevision {
                source,
                start_frame,
                end_frame,
                revision,
                update,
                assignments,
            } => {
                let memory = self.sources.entry(source.clone()).or_default();
                memory.trim(*end_frame);
                if *end_frame < memory.newest.saturating_sub(HORIZON) {
                    return;
                }
                let turns = update["turns"]
                    .as_array()
                    .filter(|t| t.len() <= 1024)
                    .and_then(|t| {
                        t.iter()
                            .map(|x| serde_json::from_value::<Turn>(x.clone()).ok())
                            .collect::<Option<Vec<_>>>()
                    })
                    .unwrap_or_default();
                let epoch = update["epoch"].as_u64().unwrap_or(0);
                // Newer overlapping evidence wins; preserve disjoint windows.
                memory.windows.retain(|w| {
                    !(w.start >= *start_frame
                        && w.end <= *end_frame
                        && (w.epoch, w.revision) <= (epoch, *revision))
                });
                memory.windows.push_back(Window {
                    start: *start_frame,
                    end: *end_frame,
                    epoch,
                    revision: *revision,
                    turns,
                });
                memory.trim(*end_frame);
                for span in &mut memory.spans {
                    if span.start < *end_frame
                        && *start_frame < span.end
                        && let Some(decision) = SourceMemory::assess(span, &memory.windows)
                    {
                        assignments.push(decision);
                    }
                }
            }
            _ => {}
        }
    }
}
