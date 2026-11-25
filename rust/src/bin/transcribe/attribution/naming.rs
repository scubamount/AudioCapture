//! Evidence-based English display names for completed local transcripts.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{Value, json};

use super::model::{Session, Turn, stable_id, valid_id};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Context {
    roster: Vec<RosterRecord>,
    prior_context: Vec<Correspondence>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RosterRecord {
    record_id: String,
    revision: u64,
    name: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Correspondence {
    session: String,
    current_speaker_id: String,
    prior_speaker_id: String,
    mapping_id: String,
    mapping_revision: u64,
    // The session loader replaces this field from the selected local manifest.
    #[serde(default)]
    snapshot: Option<Session>,
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphabetic() && !matches!(c, '-' | '\'' | '’'))
        .filter(|s| !s.is_empty())
}

fn name_shape(name: &str) -> bool {
    const EXCLUSIONS: &[&str] = &[
        "not", "no", "unknown", "someone", "the", "a", "an", "actually", "only", "also", "is",
        "never", "really",
    ];
    !name.is_empty()
        && name.len() <= 96
        && name.split(' ').count() <= 4
        && name.split(' ').all(|word| {
            word.chars().next().is_some_and(char::is_uppercase)
                && word.chars().last().is_some_and(char::is_alphabetic)
                && word
                    .chars()
                    .all(|c| c.is_alphabetic() || matches!(c, '-' | '\'' | '’'))
                && !EXCLUSIONS.contains(&word.to_ascii_lowercase().as_str())
        })
}

impl Context {
    fn read(value: &Value) -> Result<Self, String> {
        if value.is_null() {
            return Ok(Self::default());
        }
        let context =
            Self::deserialize(value).map_err(|e| format!("Invalid naming context: {e}"))?;
        let mut record_ids = BTreeSet::new();
        for r in &context.roster {
            if !valid_id(&r.record_id)
                || !record_ids.insert(&r.record_id)
                || r.revision == 0
                || !name_shape(&r.name)
                || r.text.trim().is_empty()
            {
                return Err("Invalid or repeated approved roster record".into());
            }
        }
        let mut mapping_ids = BTreeSet::new();
        for m in &context.prior_context {
            if m.session.trim().is_empty()
                || !valid_id(&m.mapping_id)
                || !mapping_ids.insert(&m.mapping_id)
                || m.mapping_revision == 0
                || !valid_id(&m.current_speaker_id)
                || !valid_id(&m.prior_speaker_id)
            {
                return Err("Invalid or repeated prior speaker correspondence".into());
            }
        }
        Ok(context)
    }

    fn canonical_roster_name(&self, text: &str) -> Option<&str> {
        self.roster
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(text))
            .map(|r| r.name.as_str())
    }
}

fn turn_text(turn: &Turn) -> &str {
    turn.reviewed_text.as_deref().unwrap_or(&turn.raw_text)
}

fn only_speaker(turn: &Turn) -> Option<&str> {
    match turn.speaker_ids.as_slice() {
        [s] if matches!(turn.attribution.as_str(), "resolved" | "reviewed") => Some(s),
        _ => None,
    }
}

fn bounds(turn: &Turn) -> Option<(u64, u64)> {
    let (start, end) = (turn.start_ms?, turn.end_ms?);
    (start < end).then_some((start, end))
}

fn strip_prefix<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.get(..prefix.len())?
        .eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

fn introduction(text: &str, context: &Context) -> Option<String> {
    // Accept a complete declarative utterance. A quotation, negation or reported
    // statement cannot start mid-sentence and borrow the speaker's identity.
    let text = text.trim().trim_end_matches(['.', '!']).trim_end();
    let greeting = ["Hello, ", "Hi, ", "Hey, ", "Hello. ", "Hi. ", "Hey. "]
        .iter()
        .find_map(|p| strip_prefix(text, p));
    let rest = greeting.unwrap_or(text);
    let explicit = strip_prefix(rest, "my name is ");
    let candidate = explicit
        .or_else(|| {
            ["I am ", "I'm ", "I’m "]
                .iter()
                .find_map(|p| strip_prefix(rest, p))
        })?
        .trim();
    let roster = context.canonical_roster_name(candidate);
    if explicit.is_none() && greeting.is_none() && roster.is_none() {
        // "I'm Ready" and "I am Reading" are not necessarily introductions.
        return None;
    }
    let candidate = roster.unwrap_or(candidate);
    name_shape(candidate).then(|| candidate.to_owned())
}

fn intro_sentences(text: &str, context: &Context) -> Vec<(String, usize, usize)> {
    let first = text.trim_start();
    let start = text.len() - first.len();
    // Treat a short standalone greeting and its introduction as one evidence span.
    let skip = ["hi. ", "hey. ", "hello. "]
        .iter()
        .find(|p| strip_prefix(first, p).is_some())
        .map_or(0, |p| p.len());
    let boundary = first[skip..]
        .find(['.', '!', '?'])
        .map_or(first.trim_end().len(), |p| skip + p + 1);
    let Some(name) = introduction(&first[..boundary], context) else {
        return vec![];
    };
    if matches!(name.as_str(), "Dr" | "Mr" | "Ms" | "Mrs" | "Prof") {
        return vec![];
    }
    let mut names = vec![(name, start, start + boundary)];
    let mut consumed = start + boundary;
    for sentence in text[consumed..].split_inclusive(['.', '!', '?']) {
        let trimmed = sentence.trim();
        let begin = consumed + sentence.len() - sentence.trim_start().len();
        consumed += sentence.len();
        let lower = trimmed.to_ascii_lowercase();
        if [
            "just kidding",
            "only kidding",
            "i was joking",
            "that was a quote",
            "that's a quote",
            "that's what",
            "that is what",
            "that was an example",
            "that's not my name",
            "i'm not ",
            "i am not ",
            "i’m not ",
            "not my name",
            "i was quoting",
            "i was reading",
            "said the ",
            "in the script",
            "he said",
            "she said",
        ]
        .iter()
        .any(|p| lower.contains(p))
        {
            return vec![];
        }
        let candidate = strip_prefix(trimmed, "actually, ")
            .or_else(|| strip_prefix(trimmed, "sorry, "))
            .unwrap_or(trimmed);
        // Even an unlisted bare name can contradict an earlier introduction.
        // This fallback only runs after that first, stronger cue exists.
        let bare_name = ["I am ", "I'm ", "I’m "]
            .iter()
            .find_map(|p| strip_prefix(candidate, p))
            .map(|v| v.trim_end_matches(['.', '!']))
            .filter(|v| name_shape(v));
        if let Some(name) =
            introduction(candidate, context).or_else(|| bare_name.map(str::to_owned))
        {
            names.push((name, begin, begin + trimmed.len()));
        } else if candidate != trimmed
            || lower.contains("my real name")
            || lower.contains("my name is ")
            || lower.contains("i meant ")
            || lower.contains("correction")
        {
            // A correction or ambiguous reported identity vetoes the earlier cue.
            return vec![];
        }
    }
    names
}

#[derive(Clone, PartialEq, Eq)]
enum Request {
    Commitment(String),
    Location(String),
}

fn location_word(word: &str) -> bool {
    matches!(
        word,
        "on" | "in"
            | "under"
            | "inside"
            | "outside"
            | "beside"
            | "behind"
            | "below"
            | "above"
            | "over"
            | "by"
            | "at"
            | "near"
            | "next"
            | "between"
            | "across"
            | "into"
            | "to"
            | "there"
            | "here"
    )
}

// Use a complete concrete noun phrase, not any one shared topic word.
fn object_and_tail(text: &str) -> Option<(&str, &str)> {
    let noun = ["the ", "our ", "your ", "my ", "these ", "those "]
        .iter()
        .find_map(|p| text.strip_prefix(p))?;
    let boundary = noun
        .split_whitespace()
        .scan(0, |offset, w| {
            let start = *offset;
            *offset += w.len() + 1;
            Some((start, w))
        })
        .find(|(_, w)| {
            location_word(w)
                || matches!(
                    *w,
                    "after"
                        | "before"
                        | "once"
                        | "when"
                        | "is"
                        | "are"
                        | "go"
                        | "goes"
                        | "belong"
                        | "belongs"
                        | "sit"
                        | "sits"
                        | "should"
                )
        })
        .map_or(noun.len(), |(i, _)| i);
    let (object, tail) = noun.split_at(boundary);
    let object = object.trim_end();
    if object.len() < 3
        || object.split_whitespace().count() > 5
        || !object
            .chars()
            .all(|c| c.is_alphabetic() || matches!(c, ' ' | '-'))
    {
        return None;
    }
    Some((object, tail))
}

fn placement(action: &str) -> Option<(&str, &str)> {
    let (_, object) = action.split_once(' ').filter(|(verb, _)| {
        matches!(
            *verb,
            "put"
                | "set"
                | "place"
                | "store"
                | "move"
                | "leave"
                | "keep"
                | "position"
                | "bring"
                | "take"
        )
    })?;
    object_and_tail(object)
}

fn directed_request(text: &str) -> Option<(&str, Request)> {
    let (name, request) = text.trim().strip_suffix('?')?.split_once(',')?;
    let name = name.trim();
    if !name_shape(name) {
        return None;
    }
    let request = request.trim_start();
    let lower = request.to_lowercase();
    for p in [
        "where should i ",
        "where should we ",
        "where can i ",
        "where can we ",
        "where do i ",
        "where do we ",
        "where is ",
        "where are ",
    ] {
        if let Some(q) = lower.strip_prefix(p) {
            let normalized = action_words(q)?;
            let (object, _) = if matches!(p, "where is " | "where are ") {
                object_and_tail(&normalized)?
            } else {
                placement(&normalized)?
            };
            return Some((name, Request::Location(object.to_owned())));
        }
    }
    let action = ["can you ", "will you ", "could you ", "would you "]
        .iter()
        .find_map(|p| strip_prefix(request, p))?;
    let action = strip_prefix(action, "please ").unwrap_or(action);
    action_words(action).map(|action| (name, Request::Commitment(action)))
}

impl Request {
    fn topic(&self) -> &str {
        match self {
            Self::Commitment(a) | Self::Location(a) => a,
        }
    }

    fn answered_by(&self, text: &str) -> bool {
        let text = text.trim();
        let text = [
            "sure. ",
            "yes. ",
            "okay. ",
            "sure, ",
            "yes, ",
            "of course, ",
        ]
        .iter()
        .find_map(|p| strip_prefix(text, p))
        .unwrap_or(text);
        let end = text.find(['.', '!', '?']).unwrap_or(text.len());
        if text.as_bytes().get(end) == Some(&b'?') {
            return false;
        }
        let lower = text.to_lowercase();
        if words(&lower).any(|w| {
            matches!(
                w,
                "no" | "not"
                    | "never"
                    | "unless"
                    | "maybe"
                    | "possibly"
                    | "might"
                    | "if"
                    | "won't"
                    | "won’t"
                    | "can't"
                    | "can’t"
                    | "cannot"
                    | "don't"
                    | "don’t"
                    | "joking"
            )
        }) || lower.contains("just kidding")
        {
            return false;
        }
        match self {
            Self::Commitment(wanted) => {
                let Some(given) = reply_action(&text[..end]) else {
                    return false;
                };
                if &given == wanted {
                    return true;
                }
                let (Some((want, dest)), Some((got, reply_dest))) =
                    (placement(wanted), placement(&given))
                else {
                    return false;
                };
                want == got
                    && !dest.is_empty()
                    && (dest == reply_dest
                        || ["here", "there", "over there", "right there"]
                            .iter()
                            .any(|p| {
                                reply_dest == *p
                                    || reply_dest.strip_prefix(p).is_some_and(|rest| {
                                        [" once ", " as soon as ", " when ", " after ", " before "]
                                            .iter()
                                            .any(|suffix| rest.starts_with(suffix))
                                    })
                            }))
            }
            Self::Location(wanted) => {
                let Some(answer) = action_words(&text[..end]) else {
                    return false;
                };
                let Some((object, predicate)) = object_and_tail(&answer) else {
                    return false;
                };
                if object != wanted {
                    return false;
                }
                [
                    "goes ",
                    "go ",
                    "is ",
                    "are ",
                    "belongs ",
                    "belong ",
                    "sits ",
                    "sit ",
                    "should go ",
                    "should be ",
                ]
                .iter()
                .filter_map(|p| predicate.strip_prefix(p))
                .any(|where_| {
                    where_.split_whitespace().next().is_some_and(location_word)
                        && (where_.split_whitespace().count() >= 2
                            || matches!(where_, "here" | "there"))
                })
            }
        }
    }
}

fn action_words(action: &str) -> Option<String> {
    if action.len() > 256
        || action
            .chars()
            .any(|c| !c.is_alphabetic() && !c.is_whitespace() && !matches!(c, '-' | '\'' | '’'))
    {
        return None;
    }
    let action = action.to_lowercase();
    let words: Vec<_> = words(&action).collect();
    if !(2..=18).contains(&words.len())
        || words.iter().any(|w| {
            matches!(
                *w,
                "never"
                    | "not"
                    | "no"
                    | "if"
                    | "unless"
                    | "or"
                    | "but"
                    | "maybe"
                    | "might"
                    | "possibly"
                    | "pretend"
            )
        })
    {
        return None;
    }
    Some(words.join(" "))
}

fn reply_action(text: &str) -> Option<String> {
    let text = text.trim().trim_end_matches(['.', '!']).trim_end();
    let text = strip_prefix(text, "yes, ")
        .or_else(|| strip_prefix(text, "sure, "))
        .unwrap_or(text);
    let action = ["I will ", "I'll ", "I’ll ", "I can ", "I shall "]
        .iter()
        .find_map(|p| strip_prefix(text, p))?;
    action_words(action)
}

fn possible_answer(text: &str, action: &str) -> bool {
    let lower = text.trim().to_lowercase();
    let mut tokens = words(&lower);
    if tokens.next().is_some_and(|w| {
        matches!(
            w,
            "yes"
                | "yeah"
                | "yep"
                | "no"
                | "nope"
                | "sure"
                | "okay"
                | "ok"
                | "me"
                | "mine"
                | "yours"
                | "i"
                | "i'll"
                | "i’ll"
                | "i'm"
                | "i’m"
                | "we"
                | "we'll"
                | "we’ll"
                | "let"
                | "absolutely"
        )
    }) {
        return true;
    }
    // A topic word alone does not make a declaration another answer. Require
    // a commitment, denial, ownership or completion predicate as well.
    let response_predicate = words(&lower).any(|w| {
        matches!(
            w,
            "mine"
                | "yours"
                | "my"
                | "myself"
                | "not"
                | "already"
                | "done"
                | "finished"
                | "ready"
                | "assigned"
                | "handled"
                | "promised"
                | "reserved"
                | "claimed"
                | "completed"
                | "sent"
                | "booked"
                | "printed"
        )
    });
    response_predicate
        && words(action)
            .filter(|w| {
                w.len() > 2
                    && !matches!(
                        *w,
                        "the"
                            | "this"
                            | "that"
                            | "those"
                            | "these"
                            | "you"
                            | "your"
                            | "our"
                            | "her"
                            | "his"
                            | "for"
                            | "with"
                            | "from"
                            | "and"
                    )
            })
            .any(|topic| words(&lower).any(|word| word == topic))
}

fn cite(session: &Session, turn: &Turn, kind: &str) -> Value {
    let text = turn_text(turn);
    let start = text.len() - text.trim_start().len();
    let end = start + text.trim().len();
    cite_span(session, turn, kind, start, end)
}

fn cite_span(session: &Session, turn: &Turn, kind: &str, start: usize, end: usize) -> Value {
    let text = &turn_text(turn)[start..end];
    let (basis, offset) = if turn.reviewed_text.is_some() {
        ("reviewed_turn", 0)
    } else {
        ("source_artifact", turn.span_bytes[0])
    };
    json!({"kind":kind,"session_id":session.session_id,"revision":session.revision,
        "source_id":turn.source_id,"artifact_id":turn.artifact_id,"turn_id":turn.turn_id,
        "span_bytes":[offset.saturating_add(start),offset.saturating_add(end)],
        "turn_span_bytes":[start,end],"basis":basis,"text":text})
}

/// Summarize intervals with two or more active speakers on each verified clock.
/// Sorting once avoids rescanning all activity intervals for each name cue.
#[derive(Default)]
struct Timeline<'a> {
    turns: Vec<&'a Turn>,
    events: Vec<(u64, &'a str, bool)>,
    overlaps: Vec<(u64, u64)>,
}

impl<'a> Timeline<'a> {
    fn push_interval(&mut self, start: u64, end: u64, who: &'a str) {
        if start < end {
            self.events.push((start, who, true));
            self.events.push((end, who, false));
        }
    }
    fn finish(&mut self) {
        self.turns
            .sort_by_key(|t| (t.start_ms, t.end_ms, &t.turn_id));
        self.events
            .sort_unstable_by_key(|(t, _, starts)| (*t, *starts));
        let mut active = BTreeMap::new();
        let mut prev = 0;
        for (point, who, starts) in self.events.drain(..) {
            if prev < point && active.len() > 1 {
                self.overlaps.push((prev, point));
            }
            if starts {
                *active.entry(who).or_insert(0usize) += 1;
            } else if let Some(count) = active.get_mut(who) {
                *count -= 1;
                if *count == 0 {
                    active.remove(who);
                }
            }
            prev = point;
        }
    }
    fn overlapping(&self, t: &Turn) -> bool {
        let Some((start, end)) = bounds(t) else {
            return false;
        };
        let i = self.overlaps.partition_point(|&(_, r)| r <= start);
        self.overlaps.get(i).is_some_and(|&(l, _)| l < end)
    }
}

fn timelines(s: &Session) -> BTreeMap<&str, Timeline<'_>> {
    let mut groups = BTreeMap::<&str, Timeline>::new();
    for t in &s.turns {
        let group = groups
            .entry(if s.shared_clock { "" } else { &t.source_id })
            .or_default();
        if let Some((start, end)) = bounds(t) {
            if turn_text(t).trim().is_empty() {
                continue;
            }
            group.turns.push(t);
            if t.speaker_ids.is_empty() {
                group.push_interval(start, end, "");
            }
            for who in &t.speaker_ids {
                group.push_interval(start, end, who);
            }
        }
    }
    for a in &s.activities {
        let group = groups
            .entry(if s.shared_clock { "" } else { &a.source_id })
            .or_default();
        group.push_interval(a.start_ms, a.end_ms, &a.speaker_id);
    }
    for group in groups.values_mut() {
        group.finish();
    }
    groups
}

struct Finding {
    name: String,
    rule: &'static str,
    sufficient: bool,
    citations: Vec<Value>,
}
type Findings = BTreeMap<String, Vec<Finding>>;

/// A cluster activity that covers several transcript utterances cannot by itself
/// attach a name cue to one specific speaker. Check positive acoustic evidence
/// before allowing a provisional name to propagate beyond the cue's turn.
fn unsupported_acoustic_turns(s: &Session) -> BTreeSet<&str> {
    let sources: BTreeSet<_> = s
        .sources
        .iter()
        .filter(|v| v.provenance["attribution_status"] == "adapter")
        .map(|v| v.source_id.as_str())
        .collect();
    let mut unsupported = BTreeSet::new();
    if sources.is_empty() {
        return unsupported;
    }
    let mut by_source: BTreeMap<&str, Vec<(u64, u64, &Turn)>> = BTreeMap::new();
    for t in &s.turns {
        if !sources.contains(t.source_id.as_str()) || turn_text(t).trim().is_empty() {
            continue;
        }
        unsupported.insert(t.turn_id.as_str());
        if let Some((start, end)) = bounds(t) {
            by_source
                .entry(&t.source_id)
                .or_default()
                .push((start, end, t));
        }
    }
    let mut activity = BTreeMap::<&str, Vec<_>>::new();
    for a in &s.activities {
        if sources.contains(a.source_id.as_str()) && a.start_ms < a.end_ms {
            activity.entry(&a.source_id).or_default().push(a);
        }
    }
    for (source_id, mut turns) in by_source {
        turns.sort_by_key(|t| t.0);
        let mut ends: Vec<_> = turns.iter().map(|t| t.1).collect();
        ends.sort_unstable();
        let mut longest = 0;
        let prefix_end: Vec<_> = turns
            .iter()
            .map(|t| {
                longest = longest.max(t.1);
                longest
            })
            .collect();
        for a in activity.get(source_id).into_iter().flatten() {
            let started = turns.partition_point(|t| t.0 < a.end_ms);
            let finished = ends.partition_point(|&e| e <= a.start_ms);
            if started - finished != 1 {
                continue;
            }
            let index = prefix_end[..started].partition_point(|&e| e <= a.start_ms);
            let (start, end, t) = turns[index];
            let covered = end.min(a.end_ms) - start.max(a.start_ms);
            if only_speaker(t) == Some(&a.speaker_id) && covered >= (end - start).div_ceil(2) {
                unsupported.remove(t.turn_id.as_str());
            }
        }
    }
    // Explicit reviewer reassignment supplies independent turn-level grounding.
    for op in &s.decisions {
        if op["op"] == "reassign"
            && let Some(turn) = op["turn_id"].as_str()
        {
            unsupported.remove(turn);
        }
    }
    unsupported
}

/// ASR can put the opening quotation mark and its contents in different turns.
/// Track the quote within the originating artifact, independent of its speaker labels.
fn quoted_continuations(s: &Session) -> BTreeSet<&str> {
    let mut turns: Vec<_> = s.turns.iter().collect();
    turns.sort_by_key(|t| (&t.source_id, &t.artifact_id, t.span_bytes[0]));
    let mut previous = None;
    let (mut double, mut single, mut reporting) = (false, false, false);
    let mut result = BTreeSet::new();
    for t in turns {
        let key = (&t.source_id, &t.artifact_id);
        if previous != Some(key) {
            (double, single, reporting) = (false, false, false);
            previous = Some(key);
        }
        if double || single || reporting {
            result.insert(t.turn_id.as_str());
        }
        let text = turn_text(t).trim();
        let mut chars = text.chars().peekable();
        let mut prev_word = false;
        while let Some(c) = chars.next() {
            let next_word = chars.peek().is_some_and(|v| v.is_alphanumeric());
            match c {
                '"' => double = !double,
                '“' => double = true,
                '”' => double = false,
                '‘' => single = true,
                '’' | '\'' if single && !next_word => single = false,
                '\'' if !prev_word && next_word => single = true,
                _ => {}
            }
            prev_word = c.is_alphanumeric();
        }
        // A reported quotation can also follow a colon with no opening mark.
        let prior = text.to_ascii_lowercase();
        reporting = prior.ends_with(':')
            && ["said", "says", "read", "wrote", "quote", "example", "line "]
                .iter()
                .any(|cue| prior.contains(cue));
    }
    result
}

fn direct(
    s: &Session,
    context: &Context,
    groups: &BTreeMap<&str, Timeline<'_>>,
    quoted: &BTreeSet<&str>,
    unsupported: &BTreeSet<&str>,
    out: &mut Findings,
) {
    for t in &s.turns {
        if quoted.contains(t.turn_id.as_str()) {
            continue;
        }
        let Some(who) = only_speaker(t) else { continue };
        let clock = if s.shared_clock { "" } else { &t.source_id };
        let disputed = groups.get(clock).is_some_and(|g| g.overlapping(t))
            || unsupported.contains(t.turn_id.as_str());
        for (name, start, end) in intro_sentences(turn_text(t), context) {
            out.entry(who.to_owned()).or_default().push(Finding {
                name,
                rule: "direct_introduction",
                sufficient: !disputed,
                citations: vec![cite_span(s, t, "introduction", start, end)],
            });
        }
    }
}

/// A direct request and its exact first-person reply should have different
/// speakers. If the acoustic cluster claims both, avoid propagating its name.
fn flag_self_replies(
    s: &Session,
    groups: &BTreeMap<&str, Timeline<'_>>,
    quoted: &BTreeSet<&str>,
    out: &mut Findings,
) {
    let acoustic: BTreeSet<_> = s
        .sources
        .iter()
        .filter(|s| s.provenance["attribution_status"] == "adapter")
        .map(|s| s.source_id.as_str())
        .collect();
    if acoustic.is_empty() {
        return;
    }
    for clock in groups.values() {
        for pair in clock.turns.windows(2) {
            let (q, r) = (pair[0], pair[1]);
            if q.source_id != r.source_id
                || !acoustic.contains(q.source_id.as_str())
                || quoted.contains(q.turn_id.as_str())
                || quoted.contains(r.turn_id.as_str())
            {
                continue;
            }
            let Some(who) = only_speaker(q) else { continue };
            let Some(candidates) = out.get_mut(who) else {
                continue;
            };
            if only_speaker(r) != Some(who) {
                continue;
            }
            let Some((_, action)) = directed_request(turn_text(q)) else {
                continue;
            };
            let (_, end) = bounds(q).unwrap();
            let (start, _) = bounds(r).unwrap();
            if !(end..=end.saturating_add(6000)).contains(&start)
                || !action.answered_by(turn_text(r))
            {
                continue;
            }
            for claim in candidates {
                claim.sufficient = false;
                claim.citations.push(cite(s, q, "speaker_conflict_request"));
                claim.citations.push(cite(s, r, "speaker_conflict_reply"));
            }
        }
    }
}

fn rejected(session: &Session, speaker: &str, name: &str) -> bool {
    session
        .decisions
        .iter()
        .rev()
        .filter(|d| {
            d["speaker_id"] == speaker
                && d["name"]
                    .as_str()
                    .is_some_and(|v| v.eq_ignore_ascii_case(name))
        })
        .find_map(|d| match d["op"].as_str()? {
            "reject" => Some(true),
            "restore_name" | "confirm" => Some(false),
            _ => None,
        })
        .unwrap_or(false)
}

fn prior(s: &Session, context: &Context, out: &mut Findings) {
    for m in &context.prior_context {
        let Some(prior) = &m.snapshot else { continue };
        if prior.session_id == s.session_id || prior.schema_major != 1 {
            continue;
        }
        let Some(speaker) = prior
            .speakers
            .iter()
            .find(|p| p.speaker_id == m.prior_speaker_id)
        else {
            continue;
        };
        let mut found = Findings::new();
        let prior_cluster_reviewed = speaker.name_status == "confirmed"
            || !unreviewed_mix_clusters(prior).contains(m.prior_speaker_id.as_str());
        let groups = timelines(prior);
        let quotes = quoted_continuations(prior);
        direct(
            prior,
            context,
            &groups,
            &quotes,
            &unsupported_acoustic_turns(prior),
            &mut found,
        );
        flag_self_replies(prior, &groups, &quotes, &mut found);
        for mut f in found.remove(&m.prior_speaker_id).unwrap_or_default() {
            if rejected(prior, &m.prior_speaker_id, &f.name) {
                continue;
            }
            f.sufficient &= prior_cluster_reviewed;
            // Other evidence in the prior session can dispute an introduction.
            f.sufficient &= !speaker.evidence.iter().any(|v| {
                v["status"] != "rejected"
                    && v["candidate"]
                        .as_str()
                        .is_some_and(|n| !n.eq_ignore_ascii_case(&f.name))
            });
            f.sufficient &= speaker.name_status != "confirmed"
                || speaker
                    .display_name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(&f.name));
            f.rule = "mapped_prior_introduction";
            f.citations.push(json!({"kind":"reviewed_correspondence","mapping_id":m.mapping_id,
                "revision":m.mapping_revision,"session_id":s.session_id,"speaker_id":m.current_speaker_id,
                "prior_session_id":prior.session_id,"prior_speaker_id":m.prior_speaker_id}));
            out.entry(m.current_speaker_id.clone()).or_default().push(f);
        }
    }
}

struct Exchange<'a> {
    asker: &'a str,
    request: &'a Turn,
    reply: &'a Turn,
    action: Request,
    clean: bool,
    isolated_request: bool,
    requester_role: Option<&'a str>,
    competing: Vec<&'a Turn>,
}

// Within one clock, accepted request/reply pairs arrive in chronological order.
// Two distinct askers and two distinct actions imply at least one pair that
// differs in both attributes. Check disjoint times before using that inference.
fn independent_exchanges(pairs: &[Exchange<'_>]) -> bool {
    if pairs.is_empty()
        || pairs.iter().any(|p| !p.clean)
        || pairs
            .windows(2)
            .any(|p| p[0].reply.end_ms > p[1].request.start_ms)
    {
        return false;
    }
    let mut isolated = pairs.iter().filter(|p| p.isolated_request);
    if let Some(first) = isolated.next()
        && isolated.clone().any(|p| p.asker != first.asker)
        && isolated.any(|p| p.action != first.action)
    {
        return true;
    }
    // A microphone and a separately captured application establish distinct
    // request origins even if one request shares an activity with a longer turn.
    // The respondent still needs independent, isolated evidence for both replies.
    let Some(first) = pairs
        .iter()
        .find(|p| !p.isolated_request && matches!(p.requester_role, Some("mic" | "program")))
    else {
        return false;
    };
    pairs.iter().any(|p| {
        matches!(
            (first.requester_role, p.requester_role),
            (Some("mic"), Some("program")) | (Some("program"), Some("mic"))
        ) && p.asker != first.asker
            && p.action != first.action
            && p.request.source_id != first.request.source_id
    })
}

fn mentions(text: &str, name: &str) -> bool {
    // Full name and word boundaries. A roster row for "Ann" cannot cite "Joanne".
    text.match_indices(name).any(|(pos, _)| {
        !text[..pos].chars().last().is_some_and(char::is_alphabetic)
            && !text[pos + name.len()..]
                .chars()
                .next()
                .is_some_and(char::is_alphabetic)
    })
}

fn independent_record(s: &Session, record: &RosterRecord, pairs: &[Exchange<'_>]) -> bool {
    if record.record_id == s.session_id
        || !mentions(&record.text, &record.name)
        || words(&record.text.to_lowercase()).any(|w| matches!(w, "not" | "never" | "absent"))
    {
        return false;
    }
    pairs.iter().flat_map(|p| [p.request, p.reply]).all(|t| {
        record.record_id != t.turn_id
            && record.record_id != t.source_id
            && record.record_id != t.artifact_id
            && !record
                .text
                .to_lowercase()
                .contains(&turn_text(t).trim().to_lowercase())
    })
}

fn conversation(
    s: &Session,
    context: &Context,
    groups: &BTreeMap<&str, Timeline<'_>>,
    quoted: &BTreeSet<&str>,
    unsupported: &BTreeSet<&str>,
    out: &mut Findings,
) {
    let persons: BTreeMap<_, _> = s
        .speakers
        .iter()
        .filter(|v| v.name_status == "confirmed")
        .filter_map(|v| Some((v.speaker_id.as_str(), v.person_id.as_deref()?)))
        .collect();
    let identity = |who| persons.get(who).copied().unwrap_or(who);
    let roles: BTreeMap<_, _> = s
        .sources
        .iter()
        .map(|v| (v.source_id.as_str(), v.role.as_str()))
        .collect();
    let mut evidence: BTreeMap<(&str, &str, &str), Vec<Exchange>> = BTreeMap::new();
    for (&clock, g) in groups {
        for (index, &request) in g.turns.iter().enumerate() {
            if quoted.contains(request.turn_id.as_str()) {
                continue;
            }
            let Some((name, action)) = directed_request(turn_text(request)) else {
                continue;
            };
            let Some(asker) = only_speaker(request) else {
                continue;
            };
            let (_, end) = bounds(request).unwrap();
            let limit = end.saturating_add(6000);
            let mut matching: Vec<(&str, &Turn)> = Vec::new();
            let mut competing = Vec::new();
            for (offset, &reply) in g.turns[index + 1..].iter().enumerate() {
                if reply.start_ms.unwrap() > limit || directed_request(turn_text(reply)).is_some() {
                    break;
                }
                if quoted.contains(reply.turn_id.as_str()) {
                    competing.push(reply);
                    continue;
                }
                if !action.answered_by(turn_text(reply)) {
                    let who = only_speaker(reply).map(identity);
                    if matching.is_empty() {
                        competing.push(reply);
                    } else if who != Some(identity(asker))
                        && !matching.iter().any(|(r, _)| Some(identity(r)) == who)
                    {
                        if possible_answer(turn_text(reply), action.topic()) {
                            competing.push(reply);
                        } else {
                            // Another speaker introduced a new topic after the direct reply.
                            break;
                        }
                    }
                    continue;
                }
                let Some(who) = only_speaker(reply) else {
                    competing.push(reply);
                    for id in &reply.speaker_ids {
                        if identity(id) != identity(asker) {
                            matching.push((id, reply));
                        }
                    }
                    continue;
                };
                if identity(who) == identity(asker) {
                    competing.push(reply);
                    continue;
                }
                if offset == 0 && reply.start_ms.unwrap() < end {
                    competing.push(reply);
                }
                matching.push((who, reply));
            }
            let clean = matching.len() == 1 && competing.is_empty() && !g.overlapping(request);
            for &(who, reply) in &matching {
                let mut competing = competing.clone();
                competing.extend(
                    matching
                        .iter()
                        .filter(|(other, _)| *other != who)
                        .map(|(_, t)| *t),
                );
                competing.sort_unstable_by_key(|t| &t.turn_id);
                competing.dedup_by_key(|t| &t.turn_id);
                evidence
                    .entry((who, name, clock))
                    .or_default()
                    .push(Exchange {
                        asker: identity(asker),
                        request,
                        reply,
                        isolated_request: !unsupported.contains(request.turn_id.as_str()),
                        requester_role: roles.get(request.source_id.as_str()).copied(),
                        action: action.clone(),
                        clean: clean
                            && !g.overlapping(reply)
                            && !unsupported.contains(reply.turn_id.as_str()),
                        competing,
                    });
            }
        }
    }
    for ((who, name, _), pairs) in evidence {
        let mut citations = Vec::new();
        for p in &pairs {
            citations.push(cite(s, p.request, "addressed_request"));
            citations.push(cite(s, p.reply, "direct_reply"));
            citations.extend(p.competing.iter().map(|t| cite(s, t, "competing_reply")));
        }
        // Repeated renderings of the same request cannot corroborate each other.
        // On separate clocks the exchanges must both occur in one source.
        let independent = independent_exchanges(&pairs);
        let records: Vec<_> = context
            .roster
            .iter()
            .filter(|r| r.name.eq_ignore_ascii_case(name) && independent_record(s, r, &pairs))
            .collect();
        for r in &records {
            citations.push(
                json!({"kind":"approved_roster","record_id":r.record_id,"revision":r.revision,
                "basis":"roster_record","span_bytes":[0,r.text.len()],"text":r.text}),
            );
        }
        out.entry(who.to_owned()).or_default().push(Finding {
            name: name.to_owned(),
            rule: "corroborated_exchange",
            sufficient: independent && !records.is_empty() && pairs.iter().all(|p| p.clean),
            citations,
        });
    }
}

/// A cue can identify one utterance without establishing who spoke every
/// utterance that a local cluster collected on an unseparated recording.
fn unreviewed_mix_clusters(s: &Session) -> BTreeSet<&str> {
    let aggregate: BTreeSet<_> = s
        .sources
        .iter()
        .filter(|v| {
            v.provenance["attribution_status"] == "adapter"
                && (matches!(v.role.as_str(), "mix" | "system")
                    || (s.sources.len() == 1 && matches!(v.role.as_str(), "program" | "other")))
        })
        .map(|v| v.source_id.as_str())
        .collect();
    let reviewed: BTreeSet<_> = s
        .decisions
        .iter()
        .filter(|d| d["op"] == "reassign")
        .filter_map(|d| d["turn_id"].as_str())
        .collect();
    s.turns
        .iter()
        .filter(|t| {
            aggregate.contains(t.source_id.as_str())
                && !reviewed.contains(t.turn_id.as_str())
                && !turn_text(t).trim().is_empty()
        })
        .flat_map(|t| t.speaker_ids.iter().map(String::as_str))
        .collect()
}

/// Recompute candidates from the current reviewed text and approved context.
/// A name never supplies a durable, cross-session person ID.
pub fn infer(session: &mut Session, config: &Value, automatic_names: bool) -> Result<(), String> {
    let context = Context::read(config)?;
    let mut findings = Findings::new();
    let groups = timelines(session);
    let quoted = quoted_continuations(session);
    let unsupported = unsupported_acoustic_turns(session);
    direct(
        session,
        &context,
        &groups,
        &quoted,
        &unsupported,
        &mut findings,
    );
    conversation(
        session,
        &context,
        &groups,
        &quoted,
        &unsupported,
        &mut findings,
    );
    prior(session, &context, &mut findings);
    flag_self_replies(session, &groups, &quoted, &mut findings);
    let unreviewed_mix = unreviewed_mix_clusters(session)
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    for i in 0..session.speakers.len() {
        let who = &session.speakers[i].speaker_id;
        let mut allowed = BTreeSet::new();
        let mut strong = None;
        let mut evidence = Vec::new();
        for f in findings.remove(who).unwrap_or_default() {
            let excluded = rejected(session, who, &f.name);
            if !excluded {
                allowed.insert(f.name.to_lowercase());
                if f.sufficient && !unreviewed_mix.contains(who) {
                    strong = Some(f.name.clone());
                }
            }
            let first = &f.citations[0];
            let evidence_id = stable_id(
                "ev",
                &[
                    &session.session_id,
                    who,
                    f.rule,
                    &f.name,
                    first["session_id"].as_str().unwrap_or_default(),
                    first["turn_id"].as_str().unwrap_or_default(),
                    f.citations
                        .last()
                        .and_then(|v| v["mapping_id"].as_str())
                        .unwrap_or_default(),
                ],
            );
            evidence.push(
                json!({"evidence_id":evidence_id,"candidate":f.name,"rule":f.rule,
                "rule_version":1,"confidence":if f.sufficient {"high"} else {"limited"},
                "status":if excluded {"rejected"} else {"suggested"},"citations":f.citations}),
            );
        }
        let speaker = &mut session.speakers[i];
        let confirmed = speaker.name_status == "confirmed" && speaker.person_id.is_some();
        if !confirmed {
            speaker.display_name = None;
            speaker.person_id = None;
            speaker.name_status = match allowed.len() {
                0 => "anonymous",
                1 => "suggested",
                _ => "ambiguous",
            }
            .to_owned();
            if automatic_names && allowed.len() == 1 && strong.is_some() {
                speaker.display_name = strong;
                speaker.name_status = "inferred".into();
            }
        }
        for item in &mut evidence {
            if item["status"] == "rejected" {
                continue;
            }
            if allowed.len() > 1
                || (confirmed && speaker.display_name.as_deref() != item["candidate"].as_str())
            {
                item["status"] = json!("conflicting");
            } else if speaker.display_name.is_some() && item["confidence"] == "high" {
                item["status"] = json!(speaker.name_status);
            }
        }
        speaker.evidence = evidence;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::model::{Session, Speaker, Turn};
    use super::*;
    use serde_json::{Value, json};

    fn conversation(lines: &[(&str, &str)]) -> Session {
        let mut s = Session {
            schema_major: 1,
            session_id: "s1".into(),
            revision: 4,
            shared_clock: true,
            ..Session::default()
        };
        for (i, &(who, text)) in lines.iter().enumerate() {
            if !s.speakers.iter().any(|s| s.speaker_id == who) {
                s.speakers.push(Speaker {
                    speaker_id: who.into(),
                    ..Speaker::default()
                });
            }
            s.turns.push(Turn {
                turn_id: format!("t{i}"),
                source_id: "src".into(),
                artifact_id: "a1".into(),
                span_bytes: [i * 200, i * 200 + text.len()],
                raw_text: text.into(),
                start_ms: Some(i as u64 * 1200),
                end_ms: Some(i as u64 * 1200 + 1000),
                speaker_ids: vec![who.into()],
                attribution: "resolved".into(),
                ..Turn::default()
            });
        }
        s
    }
    fn label<'a>(s: &'a Session, who: &str) -> Option<&'a str> {
        s.speakers
            .iter()
            .find(|p| p.speaker_id == who)
            .and_then(|p| p.display_name.as_deref())
    }
    fn roster() -> Value {
        json!({"roster":[{"record_id":"agenda","revision":2,"name":"Nia","text":"Nia hosts this review."}]})
    }
    fn exchanges() -> Session {
        conversation(&[
            ("a", "Nia, can you send the agenda?"),
            ("c", "I will send the agenda."),
            ("b", "Nia, will you host the review?"),
            ("c", "I'll host the review."),
        ])
    }
    #[test]
    fn intro_respects_display_policy_conflicts_and_revisions() {
        let mut s = conversation(&[("p", "  Hello, my name is Zoë.")]);
        infer(&mut s, &Value::Null, false).unwrap();
        assert_eq!(label(&s, "p"), None);
        assert_eq!(s.speakers[0].evidence[0]["candidate"], "Zoë");
        infer(&mut s, &Value::Null, true).unwrap();
        assert_eq!(label(&s, "p"), Some("Zoë"));
        let cite = &s.speakers[0].evidence[0]["citations"][0];
        assert_eq!(cite["turn_id"], "t0");
        assert_eq!(cite["revision"], 4);
        assert_eq!(cite["text"], "Hello, my name is Zoë.");
        s.turns[0].reviewed_text = Some("She said her name was Zoë.".into());
        infer(&mut s, &Value::Null, true).unwrap();
        assert_eq!(label(&s, "p"), None);
        let mut s = conversation(&[("p", "My name is Nia."), ("p", "My name is Jia.")]);
        infer(&mut s, &Value::Null, true).unwrap();
        assert_eq!(label(&s, "p"), None);
        assert_eq!(s.speakers[0].evidence.len(), 2);
    }
    #[test]
    fn ordinary_context_requires_two_independent_direct_exchanges() {
        let mut s = exchanges();
        infer(&mut s, &roster(), true).unwrap();
        assert_eq!(label(&s, "c"), Some("Nia"));
        let speaker = s.speakers.iter().find(|s| s.speaker_id == "c").unwrap();
        assert_eq!(speaker.person_id, None);
        assert_eq!(
            speaker.evidence[0]["citations"].as_array().unwrap().len(),
            5
        );
        for case in 0..6 {
            let mut s = exchanges();
            let mut config = roster();
            match case {
                0 => s.turns[2].speaker_ids = vec!["a".into()],
                1 => s.turns[3].raw_text = "Yes.".into(),
                2 => config = Value::Null,
                3 => s.turns[0].end_ms = Some(1400),
                4 => s.turns[2].raw_text = "Jia, will you host the review?".into(),
                _ => s.turns[1].speaker_ids.push("b".into()),
            }
            infer(&mut s, &config, true).unwrap();
            assert_eq!(label(&s, "c"), None, "case {case}");
        }
    }

    #[test]
    fn other_respondents_and_linked_requesters_block_a_contextual_name() {
        let mut s = conversation(&[
            ("a", "Nia, can you send the agenda?"),
            ("c", "I will send the agenda."),
            ("d", "Yes."),
            ("b", "Nia, will you host the review?"),
            ("c", "I'll host the review."),
        ]);
        infer(&mut s, &roster(), true).unwrap();
        assert_eq!(label(&s, "c"), None);
        let candidate = &s
            .speakers
            .iter()
            .find(|p| p.speaker_id == "c")
            .unwrap()
            .evidence[0];
        assert!(
            candidate["citations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["kind"] == "competing_reply" && v["turn_id"] == "t2")
        );
        let mut s = exchanges();
        for p in &mut s.speakers {
            if p.speaker_id == "a" || p.speaker_id == "b" {
                p.name_status = "confirmed".into();
                p.person_id = Some("person1".into());
                p.display_name = Some("Jules".into());
            }
        }
        infer(&mut s, &roster(), true).unwrap();
        assert_eq!(label(&s, "c"), None);
        assert_eq!(label(&s, "a"), Some("Jules"));
    }

    #[test]
    fn a_new_topic_after_a_complete_answer_ends_that_exchange() {
        let mut session = conversation(&[
            ("a", "Nia, could you book the room?"),
            ("c", "I can book the room."),
            ("d", "The projector needs a new lamp."),
            ("b", "Nia, will you print the charts?"),
            ("c", "I'll print the charts."),
            ("d", "A storm is forecast tomorrow."),
        ]);
        infer(&mut session, &roster(), true).unwrap();
        assert_eq!(label(&session, "c"), Some("Nia"));
        session.turns[2].raw_text = "The room has two windows.".into();
        infer(&mut session, &roster(), true).unwrap();
        assert_eq!(label(&session, "c"), Some("Nia"));
        session.turns[2].raw_text = "The room is mine.".into();
        infer(&mut session, &roster(), true).unwrap();
        assert_eq!(label(&session, "c"), None);
    }

    #[test]
    fn prior_context_requires_a_current_mapping_and_live_direct_evidence() {
        let mut session = conversation(&[("current", "Welcome.")]);
        let mut prior = conversation(&[("former", "Hi, my name is Nia!")]);
        prior.session_id = "prior".into();
        prior.revision = 3;
        let context = |s: &Session| {
            json!({"prior_context":[{
                "session":"prior.json","current_speaker_id":"current","prior_speaker_id":"former",
                "mapping_id":"map1","mapping_revision":1,"snapshot":s
            }]})
        };
        infer(&mut session, &context(&prior), true).unwrap();
        assert_eq!(label(&session, "current"), Some("Nia"));
        let cited = &session.speakers[0].evidence[0]["citations"];
        assert_eq!(cited[0]["session_id"], "prior");
        assert_eq!(cited[0]["revision"], 3);
        assert_eq!(cited[1]["mapping_id"], "map1");
        assert!(session.speakers[0].person_id.is_none());
        prior.turns[0].reviewed_text = Some("Hi, please welcome Nia.".into());
        infer(&mut session, &context(&prior), true).unwrap();
        assert!(session.speakers[0].evidence.is_empty());
        assert_eq!(label(&session, "current"), None);
    }

    #[test]
    fn displayed_candidates_obey_review_decisions_and_keep_original_ids() {
        let mut session = conversation(&[("x", "My name is Nia.")]);
        let mut d = json!({"op":"reject","speaker_id":"x","name":"Nia"});
        session.decisions.push(d.clone());
        infer(&mut session, &Value::Null, true).unwrap();
        assert_eq!(label(&session, "x"), None);
        assert_eq!(session.speakers[0].evidence[0]["status"], "rejected");
        let old_id = session.speakers[0].evidence[0]["evidence_id"].clone();
        d["op"] = json!("restore_name");
        session.decisions.push(d);
        infer(&mut session, &Value::Null, true).unwrap();
        assert_eq!(label(&session, "x"), Some("Nia"));
        assert_eq!(session.speakers[0].evidence[0]["evidence_id"], old_id);
        session.speakers[0].name_status = "confirmed".into();
        session.speakers[0].display_name = Some("Nina".into());
        session.speakers[0].person_id = Some("person1".into());
        infer(&mut session, &Value::Null, true).unwrap();
        assert_eq!(label(&session, "x"), Some("Nina"));
        assert_eq!(session.speakers[0].evidence[0]["status"], "conflicting");
    }

    #[test]
    fn reported_hypothetical_and_negated_names_never_assign_an_identity() {
        for text in [
            "Nia said she was here.",
            "They said, “My name is Nia.”",
            "If my name is Nia, then what?",
            "My name is not Nia.",
            "My name is Nia?",
            "My name is Nia, said the actor.",
            "I'm just checking the timer",
            "“I am Nia.”",
            "My name is \"Nia\".",
            "Nia, can you hear me?",
            "I'm Ready.",
        ] {
            let mut s = conversation(&[("p", text)]);
            infer(&mut s, &roster(), true).unwrap();
            assert_eq!(label(&s, "p"), None, "{text}");
        }
    }

    #[test]
    fn merged_acoustic_activity_is_insufficient_for_propagating_a_name() {
        use super::super::model::{Activity, Source};
        let mut s = conversation(&[
            ("p", "Your presentation starts at noon."),
            ("p", "My name is Nia."),
            ("p", "The bus leaves at five."),
        ]);
        s.sources.push(Source {
            source_id: "src".into(),
            provenance: json!({"attribution_status":"adapter"}),
            ..Default::default()
        });
        s.activities.push(Activity {
            source_id: "src".into(),
            speaker_id: "p".into(),
            start_ms: 0,
            end_ms: 3400,
            ..Default::default()
        });
        infer(&mut s, &Value::Null, true).unwrap();
        assert_eq!(label(&s, "p"), None);
        assert_eq!(s.speakers[0].evidence[0]["confidence"], "limited");
        s.activities[0].start_ms = 1190;
        s.activities[0].end_ms = 2200;
        infer(&mut s, &Value::Null, true).unwrap();
        assert_eq!(label(&s, "p"), Some("Nia"));
        s.turns[0].raw_text = "The actor said:".into();
        infer(&mut s, &Value::Null, true).unwrap();
        assert_eq!(label(&s, "p"), None);
        s.turns[0].raw_text = "She began to read: \"Once upon a time.".into();
        infer(&mut s, &Value::Null, true).unwrap();
        assert_eq!(label(&s, "p"), None);
    }

    #[test]
    fn self_answering_acoustic_cluster_keeps_name_as_suggestion() {
        use super::super::model::{Activity, Source};
        let mut s = conversation(&[
            ("cluster", "My name is Jules."),
            ("cluster", "Ada, will you bring the badges?"),
            ("cluster", "I will bring the badges."),
        ]);
        s.sources.push(Source {
            source_id: "src".into(),
            provenance: json!({"attribution_status":"adapter"}),
            ..Default::default()
        });
        for t in &s.turns {
            s.activities.push(Activity {
                source_id: t.source_id.clone(),
                speaker_id: "cluster".into(),
                start_ms: t.start_ms.unwrap(),
                end_ms: t.end_ms.unwrap(),
                ..Default::default()
            });
        }
        infer(&mut s, &Value::Null, true).unwrap();
        assert_eq!(label(&s, "cluster"), None);
        let e = &s.speakers[0].evidence[0];
        assert_eq!(e["candidate"], "Jules");
        assert_eq!(e["confidence"], "limited");
        assert_eq!(e["citations"][2]["kind"], "speaker_conflict_reply");
    }

    #[test]
    fn multi_sentence_intro_cites_only_the_introduction_and_detects_corrections() {
        let mut s = conversation(&[("p", "  Hi, I’m Élodie. We need fresh batteries.")]);
        infer(&mut s, &Value::Null, true).unwrap();
        assert_eq!(label(&s, "p"), Some("Élodie"));
        let c = &s.speakers[0].evidence[0]["citations"][0];
        assert_eq!(c["text"], "Hi, I’m Élodie.");
        assert_eq!(c["span_bytes"], json!([2, 2 + "Hi, I’m Élodie.".len()]));
        for bad in [
            "Hi, I’m Élodie. Actually, my name is Mia.",
            "Hi, I’m Élodie. I’m Mia.",
            "Hi, I’m Élodie. That was a quote.",
            "Hi, I’m Élodie. Just kidding.",
            "Hi, I’m Élodie. I'm not Élodie.",
        ] {
            s.turns[0].raw_text = bad.into();
            infer(&mut s, &Value::Null, true).unwrap();
            assert_eq!(label(&s, "p"), None, "{bad}");
        }
    }

    #[test]
    fn addressed_location_answers_need_an_object_and_compatible_predicate() {
        let mut s = conversation(&[
            ("a", "Nia, where should we store the spare keys?"),
            ("p", "The spare keys belong inside the desk."),
            ("b", "Nia, where are the paint brushes?"),
            ("p", "The paint brushes are on the trolley."),
        ]);
        infer(&mut s, &roster(), true).unwrap();
        assert_eq!(label(&s, "p"), Some("Nia"));
        for bad in [
            "Yes.",
            "They are there.",
            "The spare keys are shiny.",
            "The house keys belong inside the desk.",
            "The spare keys might be inside the desk.",
        ] {
            s.turns[1].raw_text = bad.into();
            infer(&mut s, &roster(), true).unwrap();
            assert_eq!(label(&s, "p"), None, "{bad}");
        }
    }

    #[test]
    fn task_pledge_can_refer_back_to_an_exact_object_and_destination() {
        let mut s = conversation(&[
            ("a", "Nia, could you place the spare spools on the shelf?"),
            (
                "p",
                "Sure. I'll move the spare spools over there once the rack is empty.",
            ),
            ("b", "Nia, where should we store the ink trays after lunch?"),
            (
                "p",
                "The ink trays go inside the cupboard. They fit on the bottom shelf.",
            ),
        ]);
        infer(&mut s, &roster(), true).unwrap();
        assert_eq!(label(&s, "p"), Some("Nia"));
        for bad in [
            "I'll move the spare spools behind the curtain.",
            "I'll move the spare spools over there instead of the shelf.",
            "I'll move the bag over there.",
            "I'll search for the spare spools over there.",
            "I’ll move the spare spools over there. Wait, I can't.",
        ] {
            s.turns[1].raw_text = bad.into();
            infer(&mut s, &roster(), true).unwrap();
            assert_eq!(label(&s, "p"), None, "{bad}");
        }
    }

    #[test]
    fn mixed_acoustic_candidates_need_review_before_cluster_wide_display() {
        use super::super::model::{Activity, Source};
        let mut s = conversation(&[("p", "My name is Ida."), ("p", "The bicycle is here.")]);
        s.sources.push(Source {
            source_id: "src".into(),
            role: "mix".into(),
            provenance: json!({"attribution_status":"adapter"}),
            ..Default::default()
        });
        for t in &s.turns {
            s.activities.push(Activity {
                source_id: t.source_id.clone(),
                speaker_id: "p".into(),
                start_ms: t.start_ms.unwrap(),
                end_ms: t.end_ms.unwrap(),
                ..Default::default()
            });
        }
        infer(&mut s, &Value::Null, true).unwrap();
        assert_eq!(label(&s, "p"), None);
        assert_eq!(s.speakers[0].evidence[0]["candidate"], "Ida");
        s.decisions
            .push(json!({"op":"reassign","turn_id":"t0","speaker_ids":["p"]}));
        infer(&mut s, &Value::Null, true).unwrap();
        assert_eq!(label(&s, "p"), None);
        s.decisions
            .push(json!({"op":"reassign","turn_id":"t1","speaker_ids":["p"]}));
        infer(&mut s, &Value::Null, true).unwrap();
        assert_eq!(label(&s, "p"), Some("Ida"));
    }

    #[test]
    fn separate_request_origins_can_support_two_isolated_responses() {
        use super::super::model::{Activity, Source};
        let mut s = conversation(&[
            ("a", "Take a seat."),
            ("a", "Nia, could you send the agenda?"),
            ("p", "I will send the agenda."),
            ("b", "Nia, will you host the review?"),
            ("p", "I will host the review."),
        ]);
        for (i, t) in s.turns.iter_mut().enumerate() {
            t.source_id = if i < 2 { "mic" } else { "program" }.into();
        }
        for id in ["mic", "program"] {
            s.sources.push(Source {
                source_id: id.into(),
                role: id.into(),
                provenance: json!({"attribution_status":"adapter"}),
                ..Default::default()
            });
        }
        s.activities.push(Activity {
            source_id: "mic".into(),
            speaker_id: "a".into(),
            start_ms: 0,
            end_ms: 2200,
            ..Default::default()
        });
        for t in &s.turns[2..] {
            s.activities.push(Activity {
                source_id: "program".into(),
                speaker_id: t.speaker_ids[0].clone(),
                start_ms: t.start_ms.unwrap(),
                end_ms: t.end_ms.unwrap(),
                ..Default::default()
            });
        }
        infer(&mut s, &roster(), true).unwrap();
        assert_eq!(label(&s, "p"), Some("Nia"));
        // Remove grounding of the second reply; a separate requester cannot replace it.
        s.activities.pop();
        infer(&mut s, &roster(), true).unwrap();
        assert_eq!(label(&s, "p"), None);
    }
}
