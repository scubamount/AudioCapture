use super::model::{Session, Speaker, Turn, stable_id, valid_id};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewPatch {
    pub session_id: String,
    pub expected_revision: u64,
    pub operations: Vec<Operation>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    CorrectText {
        turn_id: String,
        text: String,
    },
    Confirm {
        speaker_id: String,
        name: String,
        person_id: Option<String>,
    },
    Reject {
        speaker_id: String,
        name: String,
    },
    RestoreName {
        speaker_id: String,
        name: String,
    },
    Unlink {
        speaker_id: String,
    },
    Rename {
        person_id: String,
        name: String,
    },
    Reassign {
        turn_id: String,
        speaker_ids: Vec<String>,
    },
    SplitTurn {
        turn_id: String,
        at_byte: usize,
        split_ms: Option<u64>,
        reviewed_at_byte: Option<usize>,
    },
    MergeTurns {
        turn_ids: Vec<String>,
    },
    MergeSpeakers {
        from_speaker_id: String,
        into_speaker_id: String,
    },
    SplitSpeaker {
        speaker_id: String,
        turn_ids: Vec<String>,
        #[serde(default)]
        activity_ids: Vec<String>,
    },
}

/// Apply one reviewed transaction. The store holds its lock and commits the next revision.
pub fn apply(session: &Session, patch: &ReviewPatch) -> Result<Session, String> {
    if patch.session_id != session.session_id || patch.expected_revision != session.revision {
        return Err(
            "The review refers to a different session or revision. Review the latest session."
                .into(),
        );
    }
    if patch.operations.is_empty() {
        return Err("The review contains no operations.".into());
    }
    let revision = session
        .revision
        .checked_add(1)
        .ok_or("Session revision limit reached")?;
    let mut next = session.clone();
    for (index, op) in patch.operations.iter().enumerate() {
        let id = stable_id(
            "decision",
            &[
                &session.session_id,
                &revision.to_string(),
                &index.to_string(),
            ],
        );
        let mut audit = serde_json::to_value(op).map_err(|e| e.to_string())?;
        audit["session_id"] = json!(session.session_id);
        audit["revision"] = json!(revision);
        audit["decision_id"] = json!(id);
        op.execute(&mut next, &id, &mut audit)
            .map_err(|e| format!("Review operation {}: {e}", index + 1))?;
        next.decisions.push(audit);
    }
    Ok(next)
}

fn speaker_index(session: &Session, id: &str) -> Result<usize, String> {
    session
        .speakers
        .iter()
        .position(|s| s.speaker_id == id)
        .ok_or_else(|| format!("Unknown speaker ID {id:?} in this session"))
}

fn turn_index(session: &Session, id: &str) -> Result<usize, String> {
    session
        .turns
        .iter()
        .position(|t| t.turn_id == id)
        .ok_or_else(|| format!("Unknown turn ID {id:?} in this revision"))
}

fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.trim() != name || name.chars().any(char::is_control) {
        return Err(
            "Supply a display name without control characters or surrounding spaces.".into(),
        );
    }
    Ok(())
}

fn anonymous(speaker: &mut Speaker) {
    speaker.person_id = None;
    speaker.display_name = None;
    speaker.name_status = "anonymous".into();
}

fn set_turn_speakers(turn: &mut Turn, ids: Vec<String>) {
    turn.attribution = match ids.len() {
        0 => "unresolved",
        1 => "resolved",
        _ => "overlap",
    }
    .into();
    turn.speaker_ids = ids;
}

impl Operation {
    fn execute(
        &self,
        session: &mut Session,
        decision_id: &str,
        audit: &mut Value,
    ) -> Result<(), String> {
        match self {
            Self::CorrectText { turn_id, text } => {
                let index = turn_index(session, turn_id)?;
                let turn = &mut session.turns[index];
                turn.reviewed_text = (text != &turn.raw_text).then(|| text.clone());
            }
            Self::Confirm {
                speaker_id,
                name,
                person_id,
            } => {
                check_name(name)?;
                let index = speaker_index(session, speaker_id)?;
                let person = person_id
                    .as_ref()
                    .or(session.speakers[index].person_id.as_ref())
                    .cloned()
                    .unwrap_or_else(|| stable_id("person", &[decision_id]));
                if !valid_id(&person) {
                    return Err("Use a valid opaque person ID.".into());
                }
                if session.speakers.iter().any(|s| {
                    s.person_id.as_ref() == Some(&person) && s.display_name.as_ref() != Some(name)
                }) {
                    return Err(
                        "This person already has a different name. Use rename to change it.".into(),
                    );
                }
                let speaker = &mut session.speakers[index];
                audit["previous_person_id"] = json!(speaker.person_id);
                audit["person_id"] = json!(person);
                speaker.person_id = Some(person);
                speaker.display_name = Some(name.clone());
                speaker.name_status = "confirmed".into();
            }
            Self::Unlink { speaker_id } => {
                let i = speaker_index(session, speaker_id)?;
                if session.speakers[i].person_id.is_none() {
                    return Err("The speaker has no confirmed person link.".into());
                }
                audit["previous_person_id"] = json!(session.speakers[i].person_id);
                anonymous(&mut session.speakers[i]);
            }
            Self::Reject { speaker_id, name } | Self::RestoreName { speaker_id, name } => {
                check_name(name)?;
                let i = speaker_index(session, speaker_id)?;
                if matches!(self, Self::Reject { .. })
                    && session.speakers[i].display_name.as_ref() == Some(name)
                {
                    if session.speakers[i].person_id.is_some() {
                        return Err(
                            "Unlink the confirmed person before rejecting this name.".into()
                        );
                    }
                    anonymous(&mut session.speakers[i]);
                }
                // The inference pass reads the final rejection or restoration for this speaker and name.
            }
            Self::Rename { person_id, name } => {
                check_name(name)?;
                let mut matched = false;
                for speaker in &mut session.speakers {
                    if speaker.person_id.as_ref() == Some(person_id) {
                        audit["previous_name"] = json!(speaker.display_name);
                        speaker.display_name = Some(name.clone());
                        matched = true;
                    }
                }
                if !matched {
                    return Err("No speaker in this session has this confirmed person ID.".into());
                }
            }
            Self::Reassign {
                turn_id,
                speaker_ids,
            } => {
                let mut seen = HashSet::new();
                for id in speaker_ids {
                    speaker_index(session, id)?;
                    if !seen.insert(id) {
                        return Err("Repeated speaker in turn assignment.".into());
                    }
                }
                let index = turn_index(session, turn_id)?;
                set_turn_speakers(&mut session.turns[index], speaker_ids.clone());
            }
            Self::SplitTurn {
                turn_id,
                at_byte,
                split_ms,
                reviewed_at_byte,
            } => {
                split_turn(
                    session,
                    turn_id,
                    *at_byte,
                    *split_ms,
                    *reviewed_at_byte,
                    decision_id,
                    audit,
                )?;
            }
            Self::MergeTurns { turn_ids } => merge_turns(session, turn_ids, decision_id, audit)?,
            Self::MergeSpeakers {
                from_speaker_id,
                into_speaker_id,
            } => {
                merge_speakers(session, from_speaker_id, into_speaker_id, audit)?;
            }
            Self::SplitSpeaker {
                speaker_id,
                turn_ids,
                activity_ids,
            } => {
                split_speaker(
                    session,
                    speaker_id,
                    turn_ids,
                    activity_ids,
                    decision_id,
                    audit,
                )?;
            }
        }
        Ok(())
    }
}

fn split_turn(
    session: &mut Session,
    turn_id: &str,
    offset: usize,
    split_ms: Option<u64>,
    reviewed_at: Option<usize>,
    decision_id: &str,
    audit: &mut Value,
) -> Result<(), String> {
    let index = turn_index(session, turn_id)?;
    let original = &session.turns[index];
    let [start, end] = original.span_bytes;
    if end.checked_sub(start) != Some(original.raw_text.len())
        || offset == 0
        || offset >= original.raw_text.len()
        || !original.raw_text.is_char_boundary(offset)
    {
        return Err("Choose a UTF-8 boundary inside the original turn text.".into());
    }
    let (mut left, mut right) = (original.clone(), original.clone());
    let raw = original.raw_text.split_at(offset);
    left.raw_text = raw.0.into();
    right.raw_text = raw.1.into();
    left.span_bytes[1] = start + offset;
    right.span_bytes[0] = start + offset;
    match (original.reviewed_text.as_deref(), reviewed_at) {
        (Some(text), Some(at)) if text.is_char_boundary(at) => {
            let (a,b) = text.split_at(at);
            left.reviewed_text = Some(a.into());
            right.reviewed_text = Some(b.into());
        }
        (None, None) => {}
        _ => return Err("A corrected turn needs a separate valid reviewed_at_byte. Omit it for an uncorrected turn.".into()),
    }
    if let Some(at) = split_ms {
        if !matches!((original.start_ms,original.end_ms), (Some(a),Some(b)) if a < at && at < b) {
            return Err("Choose split_ms inside the original timed interval.".into());
        }
        left.end_ms = Some(at);
        right.start_ms = Some(at);
        left.timing_method = "reviewed".into();
        right.timing_method = "reviewed".into();
    } else {
        for t in [&mut left, &mut right] {
            t.start_ms = None;
            t.end_ms = None;
            t.timing_method = "untimed".into();
        }
    }
    for (turn, part) in [(&mut left, "left"), (&mut right, "right")] {
        turn.turn_id = stable_id("turn", &[decision_id, part]);
        turn.predecessors = vec![turn_id.to_owned()];
    }
    audit["successors"] = json!([left.turn_id, right.turn_id]);
    session.turns.splice(index..=index, [left, right]);
    Ok(())
}

fn merge_turns(
    session: &mut Session,
    ids: &[String],
    decision_id: &str,
    audit: &mut Value,
) -> Result<(), String> {
    if ids.len() < 2 {
        return Err("Select at least two adjacent turns in source order.".into());
    }
    let positions: HashMap<_, _> = session
        .turns
        .iter()
        .enumerate()
        .map(|(i, t)| (t.turn_id.as_str(), i))
        .collect();
    let mut indices: Vec<_> = ids
        .iter()
        .map(|id| {
            positions
                .get(id.as_str())
                .copied()
                .ok_or("Unknown turn in the requested merge")
        })
        .collect::<Result<_, _>>()?;
    if indices.iter().collect::<HashSet<_>>().len() != indices.len() {
        return Err("A turn can appear only once in a merge.".into());
    }
    let first = &session.turns[indices[0]];
    let mut merged = first.clone();
    let corrected = indices
        .iter()
        .any(|&i| session.turns[i].reviewed_text.is_some());
    let mut raw = String::new();
    let mut reviewed = String::new();
    let mut next = first.span_bytes[0];
    let mut timed = true;
    let mut previous_time = None;
    let mut same_speakers = true;
    let mut candidates = Vec::new();
    for &i in &indices {
        let t = &session.turns[i];
        if t.source_id != first.source_id
            || t.artifact_id != first.artifact_id
            || t.span_bytes[0] != next
            || t.span_bytes[1].checked_sub(t.span_bytes[0]) != Some(t.raw_text.len())
        {
            return Err("Merge contiguous original text spans from one source and artifact in source order.".into());
        }
        raw.push_str(&t.raw_text);
        if corrected {
            reviewed.push_str(t.reviewed_text.as_deref().unwrap_or(&t.raw_text));
        }
        same_speakers &= t.speaker_ids == first.speaker_ids && t.attribution == first.attribution;
        for id in &t.speaker_ids {
            if !candidates.contains(id) {
                candidates.push(id.clone());
            }
        }
        next = t.span_bytes[1];
        timed &= matches!((t.start_ms,t.end_ms), (Some(a),Some(b)) if a<=b && previous_time.is_none_or(|p| p<=a));
        previous_time = t.end_ms;
    }
    merged.raw_text = raw;
    merged.reviewed_text = corrected.then_some(reviewed);
    merged.span_bytes[1] = next;
    if !same_speakers {
        merged.speaker_ids = candidates;
        merged.attribution = "unresolved".into();
    }
    if timed {
        merged.end_ms = previous_time;
        merged.timing_method = "reviewed".into();
    } else {
        merged.start_ms = None;
        merged.end_ms = None;
        merged.timing_method = "untimed".into();
    }
    merged.turn_id = stable_id("turn", &[decision_id, "merged"]);
    merged.predecessors = ids.to_vec();
    audit["successors"] = json!([merged.turn_id]);
    indices.sort_unstable();
    let insert_at = indices[0];
    let removed: HashSet<_> = ids.iter().collect();
    session.turns.retain(|t| !removed.contains(&t.turn_id));
    session.turns.insert(insert_at, merged);
    Ok(())
}

fn replace_speaker(turn: &mut Turn, from: &str, into: &str) {
    if turn.speaker_ids.iter().any(|id| id == from) {
        let mut ids = Vec::new();
        for id in &turn.speaker_ids {
            let next = if id == from { into } else { id };
            if !ids.iter().any(|id| id == next) {
                ids.push(next.to_owned());
            }
        }
        if turn.attribution == "unresolved" {
            turn.speaker_ids = ids;
        } else {
            set_turn_speakers(turn, ids);
        }
    }
}

fn merge_speakers(
    session: &mut Session,
    from: &str,
    into: &str,
    audit: &mut Value,
) -> Result<(), String> {
    let (i, j) = (speaker_index(session, from)?, speaker_index(session, into)?);
    if i == j {
        return Err("Select two different speakers to merge.".into());
    }
    let old = &session.speakers[i];
    let keep = &session.speakers[j];
    if old.person_id.is_some() && old.person_id != keep.person_id {
        return Err("Unlink the conflicting person or confirm the same person on both speakers before merging.".into());
    }
    audit["retired_speaker_id"] = json!(from);
    for t in &mut session.turns {
        replace_speaker(t, from, into);
    }
    for a in &mut session.activities {
        if a.speaker_id == from {
            a.speaker_id = into.into();
        }
    }
    session.speakers.remove(i);
    Ok(())
}

fn split_speaker(
    session: &mut Session,
    from: &str,
    turns: &[String],
    activities: &[String],
    decision_id: &str,
    audit: &mut Value,
) -> Result<(), String> {
    speaker_index(session, from)?;
    if turns.is_empty() && activities.is_empty() {
        return Err("Select turns or activity intervals for the new speaker.".into());
    }
    let new_id = stable_id("speaker", &[decision_id, "split"]);
    let mut requested_turns: HashSet<_> = turns.iter().map(String::as_str).collect();
    let mut requested_activity: HashSet<_> = activities.iter().map(String::as_str).collect();
    if requested_turns.len() != turns.len() || requested_activity.len() != activities.len() {
        return Err("Select each turn and activity interval once.".into());
    }
    for t in &mut session.turns {
        if requested_turns.remove(t.turn_id.as_str()) {
            if !t.speaker_ids.iter().any(|id| id == from) {
                return Err("Each selected turn must belong to the original speaker.".into());
            }
            replace_speaker(t, from, &new_id);
        }
    }
    for a in &mut session.activities {
        if requested_activity.remove(a.activity_id.as_str()) {
            if a.speaker_id != from {
                return Err("Each selected interval must belong to the original speaker.".into());
            }
            a.speaker_id = new_id.clone();
        }
    }
    if !requested_turns.is_empty() || !requested_activity.is_empty() {
        return Err("Unknown turn or activity ID in this session.".into());
    }
    audit["new_speaker_id"] = json!(new_id);
    session.speakers.push(Speaker {
        speaker_id: new_id,
        name_status: "anonymous".into(),
        ..Speaker::default()
    });
    Ok(())
}

/// Render one explicitly opened local revision. JSON quoting keeps control bytes out of the terminal.
pub fn render(session: &Session) -> String {
    let mut out = String::new();
    writeln!(
        out,
        "Session {} · revision {} · {} coverage",
        session.session_id,
        session.revision,
        if session.complete {
            "complete"
        } else {
            "partial"
        }
    )
    .unwrap();
    writeln!(
        out,
        "{}",
        if session.sources.iter().any(|s| s.provenance["time_basis"] == "capture_session") {
            "Times use the capture session timeline, including recording pauses and removed silence."
        } else if session.shared_clock {
            "Times use the shared saved recording timeline."
        } else {
            "Times are relative to each source. Sources have independent timelines."
        }
    )
    .unwrap();
    for source in &session.sources {
        writeln!(
            out,
            "Source {} ({}) · {} · duration {}",
            source.source_id,
            source.role,
            source.status,
            source
                .duration_ms
                .map_or_else(|| "unknown".into(), |ms| format!("{ms} ms"))
        )
        .unwrap();
        if source.provenance["time_basis"] == "capture_session" {
            if let Some(label) = source.provenance["source_label"].as_str() {
                writeln!(out, "  Live source {label:?}").unwrap();
            }
            if let Some(refinements) = source.provenance["refinements"].as_array() {
                if refinements.is_empty() {
                    writeln!(
                        out,
                        "  Select retained audio and its timeline to verify refinement coverage."
                    )
                    .unwrap();
                }
                for pass in refinements {
                    let ranges = pass["saved_coverage_frames"]
                        .as_array()
                        .map_or(&[][..], Vec::as_slice);
                    writeln!(
                        out,
                        "  Verified {} retained ranges for recording {}.",
                        ranges.len(),
                        pass["recording"]
                    )
                    .unwrap();
                    for r in ranges.iter().take(12) {
                        if let (Some(a), Some(b)) = (r[0].as_u64(), r[1].as_u64()) {
                            writeln!(out, "    {} to {} ms", a / 48, b.div_ceil(48)).unwrap();
                        }
                    }
                    if ranges.len() > 12 {
                        writeln!(out, "    Use --json for all retained ranges.").unwrap();
                    }
                }
            }
        }
        if let Some(status) = source.provenance["attribution_status"].as_str() {
            writeln!(out, "  Speaker attribution {status:?}").unwrap();
        }
        if let Some(error) = source.provenance["adapter_error"].as_str() {
            writeln!(out, "  Adapter error {error:?}").unwrap();
        }
        for gap in &source.gaps {
            if gap["kind"] != "diarization" {
                writeln!(out, "  Source coverage {gap}").unwrap();
            }
        }
    }
    let speakers: HashMap<_, _> = session
        .speakers
        .iter()
        .map(|s| (s.speaker_id.as_str(), s))
        .collect();
    out.push_str("\nSpeakers\n");
    for speaker in &session.speakers {
        write!(out, "{} · {}", speaker.speaker_id, speaker.name_status).unwrap();
        if let Some(name) = &speaker.display_name {
            write!(out, " · {name:?}").unwrap();
        }
        if let Some(person) = &speaker.person_id {
            write!(out, " · person {person}").unwrap();
        }
        out.push('\n');
        for e in &speaker.evidence {
            writeln!(
                out,
                "  Candidate {} · {} · confidence {} · rule {}",
                e["candidate"], e["status"], e["confidence"], e["rule"]
            )
            .unwrap();
            for cite in e["citations"].as_array().into_iter().flatten() {
                writeln!(out, "    {cite}").unwrap();
            }
        }
    }
    out.push_str("\nTurns\n");
    // The session store already orders shared and independent timelines.
    for t in &session.turns {
        let when = t
            .start_ms
            .zip(t.end_ms)
            .map_or_else(|| "untimed".into(), |(a, b)| format!("{a}–{b} ms"));
        let who = if t.speaker_ids.is_empty() {
            "unknown".into()
        } else {
            t.speaker_ids
                .iter()
                .map(|id| {
                    match speakers
                        .get(id.as_str())
                        .and_then(|s| s.display_name.as_ref())
                    {
                        Some(name) => format!("{id} ({name:?})"),
                        None => id.clone(),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        writeln!(
            out,
            "{} · {} · {} · {} · {}",
            t.turn_id, t.source_id, when, who, t.attribution
        )
        .unwrap();
        writeln!(
            out,
            "  {:?}",
            t.reviewed_text.as_deref().unwrap_or(&t.raw_text)
        )
        .unwrap();
        if t.reviewed_text.is_some() {
            writeln!(out, "  Original {:?}", t.raw_text).unwrap();
        }
    }
    if !session.activities.is_empty() {
        out.push_str("\nSpeaker activity\n");
    }
    for a in &session.activities {
        writeln!(
            out,
            "{} · {} · {}–{} ms · {}",
            a.activity_id, a.source_id, a.start_ms, a.end_ms, a.speaker_id
        )
        .unwrap();
    }
    writeln!(
        out,
        "\n{} recorded review decisions.",
        session.decisions.len()
    )
    .unwrap();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn apply(session: &mut Session, patch: &ReviewPatch) -> Result<(), String> {
        *session = super::apply(session, patch)?;
        Ok(())
    }
    fn sample() -> Session {
        Session {
            session_id: "s1".into(),
            revision: 7,
            speakers: vec![Speaker {
                speaker_id: "sp1".into(),
                name_status: "anonymous".into(),
                ..Speaker::default()
            }],
            turns: vec![Turn {
                turn_id: "t1".into(),
                source_id: "a1".into(),
                artifact_id: "r1".into(),
                span_bytes: [12, 19],
                raw_text: "é wait".into(),
                speaker_ids: vec!["sp1".into()],
                attribution: "resolved".into(),
                start_ms: Some(10),
                end_ms: Some(200),
                ..Turn::default()
            }],
            ..Session::default()
        }
    }
    fn patch(ops: Value) -> ReviewPatch {
        serde_json::from_value(json!({"session_id":"s1","expected_revision":7,"operations":ops}))
            .unwrap()
    }
    #[test]
    fn corrections_keep_the_canonical_text_and_expected_revision() {
        let mut session = sample();
        let ops = patch(json!([{"op":"correct_text","turn_id":"t1","text":"Please wait."}]));
        apply(&mut session, &ops).unwrap();
        assert_eq!(session.turns[0].raw_text, "é wait");
        assert_eq!(
            session.turns[0].reviewed_text.as_deref(),
            Some("Please wait.")
        );
        assert_eq!(session.decisions[0]["revision"], 8);
        assert_eq!(session.revision, 7);
    }

    #[test]
    fn failed_or_stale_review_preserves_every_field() {
        let mut s = sample();
        let before = serde_json::to_value(&s).unwrap();
        let ops = patch(json!([
            {"op":"correct_text","turn_id":"t1","text":"Reviewed"},
            {"op":"reassign","turn_id":"t1","speaker_ids":["outside-session"]}
        ]));
        assert!(apply(&mut s, &ops).is_err());
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
        let mut ops = patch(json!([{"op":"unlink","speaker_id":"sp1"}]));
        ops.expected_revision = 6;
        assert!(apply(&mut s, &ops).is_err());
        ops.expected_revision = 7;
        ops.session_id = "different".into();
        assert!(apply(&mut s, &ops).is_err());
        assert!(apply(&mut s, &patch(json!([]))).is_err());
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
    }

    #[test]
    fn split_and_merge_keep_raw_and_reviewed_spans_and_publish_lineage() {
        let mut s = sample();
        s.turns[0].reviewed_text = Some("Ë Wait.".into());
        let split = |at, reviewed| {
            patch(json!([{"op":"split_turn","turn_id":"t1",
            "at_byte":at,"reviewed_at_byte":reviewed,"split_ms":50}]))
        };
        assert!(apply(&mut s, &split(1, Some(2))).is_err());
        assert!(apply(&mut s, &split(2, None::<i32>)).is_err());
        assert!(apply(&mut s, &split(2, Some(1))).is_err());
        apply(&mut s, &split(2, Some(2))).unwrap();
        assert_eq!(s.turns.len(), 2);
        assert_eq!(s.turns[0].predecessors, ["t1"]);
        assert_eq!(s.turns[1].predecessors, ["t1"]);
        assert_eq!(s.turns[0].span_bytes, [12, 14]);
        assert_eq!(s.turns[1].span_bytes, [14, 19]);
        assert_eq!(s.turns[0].end_ms, Some(50));
        assert_eq!(s.turns[1].start_ms, Some(50));
        let ids = vec![s.turns[0].turn_id.clone(), s.turns[1].turn_id.clone()];
        assert_ne!(ids[0], ids[1]);
        s.revision += 1;
        let mut join = patch(json!([{"op":"merge_turns","turn_ids":ids}]));
        join.expected_revision = s.revision;
        apply(&mut s, &join).unwrap();
        assert_eq!(s.turns.len(), 1);
        let merged = &s.turns[0];
        assert_eq!(merged.predecessors, ids);
        assert_eq!(merged.raw_text, "é wait");
        assert_eq!(merged.reviewed_text.as_deref(), Some("Ë Wait."));
        assert_eq!(merged.span_bytes, [12, 19]);
        assert_eq!((merged.start_ms, merged.end_ms), (Some(10), Some(200)));
        assert_ne!(merged.turn_id, "t1");
        assert_eq!(s.decisions.len(), 2);
    }

    #[test]
    fn split_without_reviewed_time_creates_no_word_boundaries() {
        let mut s = sample();
        apply(
            &mut s,
            &patch(json!([{"op":"split_turn","turn_id":"t1","at_byte":2}])),
        )
        .unwrap();
        assert!(
            s.turns
                .iter()
                .all(|t| t.start_ms.is_none() && t.end_ms.is_none())
        );
    }

    #[test]
    fn merge_rejects_cross_source_or_noncontiguous_spans() {
        let mut s = sample();
        let mut other = s.turns[0].clone();
        other.turn_id = "t2".into();
        other.source_id = "other".into();
        other.span_bytes = [19, 26];
        s.turns.push(other);
        let join = patch(json!([{"op":"merge_turns","turn_ids":["t1","t2"]}]));
        assert!(apply(&mut s, &join).is_err());
        s.turns[1].source_id = "a1".into();
        s.turns[1].span_bytes = [21, 28];
        assert!(apply(&mut s, &join).is_err());
    }

    #[test]
    fn person_changes_preserve_anonymous_ids_and_rejection_history() {
        let mut s = sample();
        s.speakers.push(Speaker {
            speaker_id: "sp2".into(),
            ..Speaker::default()
        });
        apply(
            &mut s,
            &patch(json!([
                {"op":"confirm","speaker_id":"sp1","name":"Nia","person_id":"p1"},
                {"op":"confirm","speaker_id":"sp2","name":"Nia","person_id":"p1"},
                {"op":"rename","person_id":"p1","name":"Nia T."},
                {"op":"unlink","speaker_id":"sp2"},
                {"op":"reject","speaker_id":"sp2","name":"Nia"},
                {"op":"restore_name","speaker_id":"sp2","name":"Nia"}
            ])),
        )
        .unwrap();
        assert_eq!(s.speakers[0].display_name.as_deref(), Some("Nia T."));
        assert_eq!(s.speakers[0].person_id.as_deref(), Some("p1"));
        assert_eq!(s.speakers[1].person_id, None);
        assert_eq!(s.speakers[1].speaker_id, "sp2");
        assert_eq!(s.turns[0].speaker_ids, ["sp1"]);
        assert_eq!(s.decisions[3]["previous_person_id"], "p1");
        assert_eq!(s.decisions[4]["op"], "reject");
        assert_eq!(s.decisions[5]["op"], "restore_name");
        assert_eq!(
            s.decisions
                .iter()
                .map(|v| v["decision_id"].as_str().unwrap())
                .collect::<HashSet<_>>()
                .len(),
            6
        );
    }

    #[test]
    fn split_and_merge_speakers_keep_activity_and_turn_ids() {
        let mut s = sample();
        s.activities.push(super::super::model::Activity {
            activity_id: "act".into(),
            source_id: "a1".into(),
            speaker_id: "sp1".into(),
            start_ms: 10,
            end_ms: 200,
            ..Default::default()
        });
        apply(&mut s,&patch(json!([{"op":"split_speaker","speaker_id":"sp1","turn_ids":["t1"],"activity_ids":["act"]}]))).unwrap();
        let other = s.turns[0].speaker_ids[0].clone();
        assert_eq!(s.activities[0].speaker_id, other);
        assert_eq!(s.speakers[1].person_id, None);
        s.revision += 1;
        let mut join =
            patch(json!([{"op":"merge_speakers","from_speaker_id":other,"into_speaker_id":"sp1"}]));
        join.expected_revision = s.revision;
        apply(&mut s, &join).unwrap();
        assert_eq!(s.speakers.len(), 1);
        assert_eq!(s.turns[0].speaker_ids, ["sp1"]);
        assert_eq!(s.turns[0].turn_id, "t1");
        assert_eq!(s.activities[0].speaker_id, "sp1");
        assert_eq!(s.activities[0].activity_id, "act");
    }

    #[test]
    fn review_shows_evidence_coverage_unresolved_and_both_text_versions() {
        let mut s = sample();
        s.turns[0].reviewed_text = Some("Correction.".into());
        s.turns[0].speaker_ids.clear();
        s.turns[0].attribution = "unresolved".into();
        s.turns[0].start_ms = None;
        s.turns[0].end_ms = None;
        s.sources.push(super::super::model::Source {
            source_id:"a1".into(), status:"untimed".into(), role:"mic".into(),
            provenance:json!({"attribution_status":"adapter_failed","adapter_error":"Missing backend"}),
            gaps:vec![json!({"kind":"diarization","message":"Missing backend"})],..Default::default()
        });
        s.speakers[0].evidence.push(json!({"candidate":"Nia","confidence":"high","citations":[{"turn_id":"sample-id","text":"\u{1b}[31mword"}]}));
        let view = render(&s);
        for item in [
            "partial",
            "Correction.",
            "Original",
            "é wait",
            "untimed",
            "unknown",
            "Nia",
            "high",
            "sample-id",
            "adapter_failed",
            "Adapter error",
        ] {
            assert!(view.contains(item), "Missing {item}");
        }
        assert!(!view.contains(char::from(27)));
        assert!(!view.contains("Source coverage"));
    }
}
