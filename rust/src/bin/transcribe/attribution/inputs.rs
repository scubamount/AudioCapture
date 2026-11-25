//! Read local recognition artifacts and align their original UTF-8 spans to source activity.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

// Preserve pauses long enough for a reply on a separate source track.
const MAX_WORD_GROUP_GAP_MS: u64 = 500;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Interval {
    pub label: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Span {
    pub bytes: [usize; 2],
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub labels: Vec<String>,
    pub timing_method: String,
    group: usize,
}

#[derive(Clone, Debug)]
struct Unit {
    bytes: [usize; 2],
    time: Option<(u64, u64)>,
    method: &'static str,
    speaker: Option<String>,
    group: usize,
}

#[derive(Debug, Default)]
pub struct Recognition {
    pub text: String,
    pub spans: Vec<Span>,
    pub activity: Vec<Interval>,
    pub model: Option<String>,
    pub issues: Vec<String>,
    pub gaps: Vec<([u64; 2], String)>,
    pub provider_profile: Option<String>,
    units: Vec<Unit>,
}

impl Recognition {
    fn gap(&mut self, start: u64, end: u64) {
        let message = format!("Missing chunk interval {start}..{end} ms");
        self.issues.push(message.clone());
        self.gaps.push(([start, end], message));
    }
    fn add_plain(&mut self, text: &str, group: usize) {
        if text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(text);
        self.units.push(Unit {
            bytes: [start, self.text.len()],
            time: None,
            method: "untimed",
            speaker: None,
            group,
        });
    }

    fn render(&mut self, provider: bool) {
        self.activity.sort_unstable_by(|a, b| {
            (a.start_ms, a.end_ms, &a.label).cmp(&(b.start_ms, b.end_ms, &b.label))
        });
        self.activity.dedup();
        // End times need not be sorted when speakers overlap. Their running
        // maximum lets each turn skip expired intervals by binary search.
        let mut end = 0;
        let ends: Vec<_> = self
            .activity
            .iter()
            .map(|i| {
                end = end.max(i.end_ms);
                end
            })
            .collect();
        self.spans.clear();
        let mut previous_group = None;
        for unit in &self.units {
            let (start_ms, end_ms) = unit.time.unzip();
            let labels = match (&unit.speaker, unit.time) {
                (Some(speaker), Some(_)) if provider => vec![speaker.clone()],
                // An unnamed provider segment can overlap a second, identified
                // speaker. Only an explicit adapter may independently assign it.
                (None, _) if provider => Vec::new(),
                (_, Some((start, end))) => active_labels(&self.activity, &ends, start, end),
                _ => Vec::new(),
            };
            if previous_group == Some(unit.group)
                && let Some(prev) = self.spans.last_mut()
                && prev.timing_method == "word"
                && unit.method == "word"
                && prev.labels == labels
                && prev.bytes[1] == unit.bytes[0]
                && let (Some(old_end), Some(start), Some(end)) = (prev.end_ms, start_ms, end_ms)
                && start <= old_end.saturating_add(MAX_WORD_GROUP_GAP_MS)
                && prev.start_ms.is_some_and(|s| start >= s)
            {
                prev.bytes[1] = unit.bytes[1];
                prev.end_ms = Some(old_end.max(end));
            } else {
                self.spans.push(Span {
                    bytes: unit.bytes,
                    start_ms,
                    end_ms,
                    labels,
                    timing_method: unit.method.into(),
                    group: unit.group,
                });
            }
            previous_group = Some(unit.group);
        }
        if !provider && self.spans.len() > 1 && !self.activity.is_empty() {
            clear_competing_assignments(&self.text, &mut self.spans);
        }
    }
}

/// One acoustic cluster cannot identify two independent, substantially concurrent
/// ASR turns. Exclude brief alignment jitter and duplicate recognition text.
fn clear_competing_assignments(text: &str, spans: &mut [Span]) {
    const JITTER_MS: u64 = 250;
    let mut timed: Vec<_> = spans
        .iter()
        .enumerate()
        .filter_map(|(index, span)| {
            let (start, end) = (span.start_ms?, span.end_ms?);
            (span.labels.len() == 1 && end.saturating_sub(start) >= JITTER_MS)
                .then_some((start, end, index))
        })
        .collect();
    if timed.len() < 2 {
        return;
    }
    timed.sort_unstable();
    let mut uncertain = vec![false; spans.len()];
    for (pos, &(start, end, a)) in timed.iter().enumerate() {
        for &(next, finish, b) in &timed[pos + 1..] {
            if next > end - JITTER_MS {
                break;
            }
            if spans[a].group == spans[b].group
                || spans[a].labels != spans[b].labels
                || (uncertain[a] && uncertain[b])
            {
                continue;
            }
            let overlap = end.min(finish) - next;
            let shorter = (end - start).min(finish - next);
            if overlap < JITTER_MS || overlap < shorter.div_ceil(2) {
                continue;
            }
            let wording = |s: &Span| {
                text[s.bytes[0]..s.bytes[1]]
                    .chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(char::to_lowercase)
            };
            if !wording(&spans[a]).eq(wording(&spans[b])) {
                uncertain[a] = true;
                uncertain[b] = true;
            }
        }
    }
    for (span, unknown) in spans.iter_mut().zip(uncertain) {
        if unknown {
            span.labels.clear();
        }
    }
}

fn active_labels(activity: &[Interval], ends: &[u64], start: u64, end: u64) -> Vec<String> {
    // A zero-duration word has a location but supplies no speech duration.
    if end <= start {
        return Vec::new();
    }
    let lo = ends.partition_point(|&e| e <= start);
    let hi = activity.partition_point(|i| i.start_ms < end);
    let mut labels = BTreeSet::new();
    let mut covered = 0u64;
    let mut latest = start;
    for item in &activity[lo..hi] {
        if item.end_ms <= start {
            continue;
        }
        labels.insert(item.label.as_str());
        let right = item.end_ms.min(end);
        covered += right.saturating_sub(latest.max(item.start_ms));
        latest = latest.max(right);
    }
    if labels.len() == 1 && covered < (end - start).div_ceil(2) {
        return Vec::new();
    }
    labels.into_iter().map(str::to_owned).collect()
}

/// Replace provider activity with the explicitly selected adapter's source-level intervals.
pub fn align(rec: &mut Recognition, activity: Vec<Interval>) {
    rec.activity = activity;
    rec.activity
        .retain(|i| i.end_ms > i.start_ms && !i.label.is_empty());
    rec.render(false);
}

fn local_range(item: &Value, duration: Option<u64>) -> Option<(u64, u64)> {
    let duration = duration?;
    let start = item.get("start")?.as_f64()?;
    let end = item.get("end")?.as_f64()?;
    if !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || end < start
        || (end * 1000.).round() > duration as f64
    {
        return None;
    }
    Some(((start * 1000.).round() as u64, (end * 1000.).round() as u64))
}

fn speaker(item: &Value, namespace: &str) -> Option<String> {
    let name = item.get("speaker")?.as_str()?.trim();
    if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return None;
    }
    Some(format!("{namespace}{name}"))
}

/// Compare only text and whitespace. Preserve byte positions in the source, including
/// leading separators, trailing whitespace and the original form of Unicode.
fn match_texts(
    text: &str,
    items: &[Value],
    field: &str,
    prefixed: bool,
) -> Option<Vec<[usize; 3]>> {
    if items.is_empty() {
        return None;
    }
    let mut spans = Vec::with_capacity(items.len());
    let mut cursor = 0;
    for item in items {
        let token = item.get(field)?.as_str()?.trim();
        if token.is_empty() {
            return None;
        }
        let start = cursor;
        cursor = text.len() - text[cursor..].trim_start().len();
        let mut content = start;
        if prefixed && let Some(label) = item["speaker"].as_str().filter(|s| !s.is_empty()) {
            let rest = &text[cursor..];
            if rest
                .get(..label.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(label))
                && rest.as_bytes().get(label.len()) == Some(&b':')
            {
                cursor += label.len() + 1;
                cursor = text.len() - text[cursor..].trim_start().len();
                content = cursor;
            }
        }
        let mut words = token.split_whitespace();
        let mut word = words.next()?;
        loop {
            if !text[cursor..].starts_with(word) {
                return None;
            }
            cursor += word.len();
            let Some(next) = words.next() else {
                break;
            };
            let after = text.len() - text[cursor..].trim_start().len();
            if after == cursor {
                return None;
            }
            cursor = after;
            word = next;
        }
        spans.push([start, content, cursor]);
    }
    if !text[cursor..].trim().is_empty() {
        return None;
    }
    spans.last_mut()?[2] = text.len();
    Some(spans)
}

fn offset_range(time: Option<(u64, u64)>, offset: u64) -> Option<(u64, u64)> {
    let (start, end) = time?;
    Some((offset.checked_add(start)?, offset.checked_add(end)?))
}

fn collect_activity(
    out: &mut Vec<Interval>,
    items: &[Value],
    duration: Option<u64>,
    offset: u64,
    namespace: &str,
) {
    for item in items {
        if let (Some(label), Some((start_ms, end_ms))) = (
            speaker(item, namespace),
            offset_range(local_range(item, duration), offset),
        ) && end_ms > start_ms
        {
            out.push(Interval {
                label,
                start_ms,
                end_ms,
            });
        }
    }
}

struct Part<'a> {
    value: &'a Value,
    duration: Option<u64>,
    offset_ms: u64,
    namespace: &'a str,
}

impl Part<'_> {
    fn word_units(
        &self,
        words: &[Value],
        text: &str,
        base: usize,
        group: usize,
        parent: Option<&str>,
    ) -> Option<Vec<Unit>> {
        let ranges = match_texts(text, words, "word", false)?;
        words
            .iter()
            .zip(ranges)
            .map(|(word, [left, _, right])| {
                let time = offset_range(local_range(word, self.duration), self.offset_ms)?;
                Some(Unit {
                    bytes: [base + left, base + right],
                    time: Some(time),
                    method: "word",
                    group,
                    speaker: speaker(word, self.namespace).or_else(|| parent.map(str::to_owned)),
                })
            })
            .collect()
    }

    fn append(&self, out: &mut Recognition) -> Result<(), String> {
        let text = self.value["text"]
            .as_str()
            .ok_or("Recognition JSON has no text field")?;
        let group = out.units.len();
        let base = out.text.len();
        let candidates = [
            self.value["speaker_segments"].as_array(),
            self.value["segments"].as_array(),
        ];
        let mut projected = BTreeMap::new();
        for item in candidates[0].into_iter().flatten() {
            if let (Some(t), Some(s), Some(label)) = (
                local_range(item, self.duration),
                item["text"].as_str(),
                speaker(item, self.namespace),
            ) {
                let entry = projected
                    .entry((t, s.trim()))
                    .or_insert_with(|| Some(label.clone()));
                if entry.as_deref() != Some(label.as_str()) {
                    *entry = None;
                }
            }
        }
        let (segments, parts) = candidates
            .iter()
            .flatten()
            .find_map(|segs| {
                match_texts(text, segs, "text", false)
                    .or_else(|| match_texts(text, segs, "text", true))
                    .map(|ranges| (Some(*segs), Some(ranges)))
            })
            .unwrap_or((None, None));
        let words = self.value["words"].as_array();
        for segs in candidates.into_iter().flatten() {
            collect_activity(
                &mut out.activity,
                segs,
                self.duration,
                self.offset_ms,
                self.namespace,
            );
            for nested in segs.iter().filter_map(|s| s["words"].as_array()) {
                collect_activity(
                    &mut out.activity,
                    nested,
                    self.duration,
                    self.offset_ms,
                    self.namespace,
                );
            }
        }
        if let Some(words) = words {
            collect_activity(
                &mut out.activity,
                words,
                self.duration,
                self.offset_ms,
                self.namespace,
            );
        }
        // Some providers supply words only at the top level. Use those verified
        // spans and preserve segment group boundaries whenever they are available.
        if !segments.is_some_and(|s| s.iter().any(|v| v["words"].is_array()))
            && let Some(mut units) = words.and_then(|w| self.word_units(w, text, base, group, None))
        {
            if let (Some(parts), Some(segments)) = (&parts, segments) {
                for u in &mut units {
                    let index = parts.partition_point(|p| base + p[2] <= u.bytes[0]);
                    if let Some(range) = parts.get(index)
                        && u.bytes[0] >= base + range[1]
                        && u.bytes[1] <= base + range[2]
                    {
                        u.group += index;
                        if u.speaker.is_none()
                            && local_range(&segments[index], self.duration).is_some()
                        {
                            u.speaker = speaker(&segments[index], self.namespace);
                        }
                    }
                }
            }
            out.text.push_str(text);
            out.units.extend(units);
            return Ok(());
        }
        let (Some(segments), Some(parts)) = (segments, parts) else {
            out.add_plain(text, group);
            return Ok(());
        };
        out.text.push_str(text);
        for (index, (segment, [start, left, right])) in segments.iter().zip(parts).enumerate() {
            if start != left {
                out.units.push(Unit {
                    bytes: [base + start, base + left],
                    time: None,
                    method: "untimed",
                    speaker: None,
                    group: group + index,
                });
            }
            let time = offset_range(local_range(segment, self.duration), self.offset_ms);
            let label = local_range(segment, self.duration).and_then(|range| {
                speaker(segment, self.namespace).or_else(|| {
                    projected
                        .get(&(range, segment["text"].as_str()?.trim()))
                        .cloned()
                        .flatten()
                })
            });
            let words = segment["words"].as_array().and_then(|w| {
                self.word_units(
                    w,
                    &text[left..right],
                    base + left,
                    group + index,
                    label.as_deref(),
                )
            });
            if let Some(units) = words {
                out.units.extend(units);
            } else {
                out.units.push(Unit {
                    bytes: [base + left, base + right],
                    time,
                    speaker: label,
                    group: group + index,
                    method: if time.is_some() { "segment" } else { "untimed" },
                });
            }
        }
        Ok(())
    }
}

fn parse_provider(value: &Value, duration: Option<u64>) -> Result<Recognition, String> {
    let mut r = Recognition {
        model: value["model"].as_str().map(str::to_owned),
        provider_profile: value["provider_profile"].as_str().map(str::to_owned),
        ..Default::default()
    };
    Part {
        value,
        duration,
        offset_ms: 0,
        namespace: "",
    }
    .append(&mut r)?;
    r.render(true);
    Ok(r)
}

fn frame_ms(frames: u64, rate: u64) -> Option<u64> {
    let ms = (u128::from(frames) * 1000 + u128::from(rate) / 2) / u128::from(rate);
    ms.try_into().ok()
}

fn check_audio(value: &Value, expected: &str) -> Result<(), String> {
    if value["source_sha256"]
        .as_str()
        .is_some_and(|v| v.eq_ignore_ascii_case(expected))
    {
        Ok(())
    } else {
        Err("Recognition artifact belongs to different audio bytes".into())
    }
}

fn parse_aggregate(
    value: &Value,
    expected: &str,
    duration: Option<u64>,
) -> Result<Recognition, String> {
    check_audio(value, expected)?;
    let parts = value["parts"].as_array().ok_or("Aggregate lacks parts")?;
    let rate = value["sample_rate"]
        .as_u64()
        .filter(|r| *r == 16000)
        .ok_or("Unsupported aggregate sample rate")?;
    let mut r = Recognition {
        model: value["model"].as_str().map(str::to_owned),
        provider_profile: value["provider_profile"].as_str().map(str::to_owned),
        ..Default::default()
    };
    let mut next = 0;
    for (i, part) in parts.iter().enumerate() {
        let offset = part["offset_frames"]
            .as_u64()
            .ok_or("Missing chunk offset")?;
        let frames = part["frames"]
            .as_u64()
            .filter(|f| *f > 0)
            .ok_or("Invalid chunk length")?;
        if part["sample_rate"].as_u64() != Some(rate) || offset < next {
            return Err("Overlapping, unordered, or invalid aggregate chunks".into());
        }
        let start = frame_ms(offset, rate).ok_or("Invalid chunk offset")?;
        if offset > next {
            r.gap(frame_ms(next, rate).ok_or("Invalid chunk offset")?, start);
        }
        next = offset.checked_add(frames).ok_or("Invalid chunk length")?;
        let end = frame_ms(next, rate).ok_or("Invalid chunk end")?;
        if !part["text"].is_string() {
            r.gap(start, end);
            continue;
        }
        if !r.text.is_empty() && part["text"].as_str().is_some_and(|t| !t.is_empty()) {
            r.add_plain("\n", r.units.len());
        }
        let local_duration = duration.unwrap_or(end).min(end).checked_sub(start);
        Part {
            value: part,
            duration: local_duration,
            offset_ms: start,
            namespace: &format!("part-{i}/"),
        }
        .append(&mut r)?;
    }
    if let Some(total) = duration {
        let end = frame_ms(next, rate).ok_or("Invalid aggregate length")?;
        // Container duration can include one codec frame of padding after decoded audio.
        if end.saturating_add(100) < total {
            r.gap(end, total);
        }
    }
    if parts.is_empty() {
        r.issues
            .push("Aggregate contains no completed chunks".into());
    }
    r.render(true);
    Ok(r)
}

fn read_json(path: &Path) -> Result<Value, String> {
    let file = File::open(path).map_err(|e| format!("Cannot open recognition JSON: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Recognition JSON must be a regular file".into());
    }
    serde_json::from_reader(BufReader::new(file))
        .map_err(|e| format!("Invalid recognition JSON: {e}"))
}

fn read_text(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("Cannot read UTF-8 recognition text: {e}"))
}

fn plain(text: String) -> Recognition {
    let mut r = Recognition::default();
    if !text.is_empty() {
        r.units.push(Unit {
            bytes: [0, text.len()],
            time: None,
            method: "untimed",
            speaker: None,
            group: 0,
        });
    }
    r.text = text;
    r.render(true);
    r
}

fn directory_spelling(path: &Path) -> Option<String> {
    let name = path.to_str()?.replace('/', "\\").to_lowercase();
    Some(match name.strip_prefix(r"\\?\unc\") {
        Some(unc) => format!(r"\\{unc}"),
        None => name.strip_prefix(r"\\?\").unwrap_or(&name).to_owned(),
    })
}

fn short_directory_name(root: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

    let root: Vec<_> = root.as_os_str().encode_wide().chain(Some(0)).collect();
    // Ask Windows for aliases of the trusted root only. Never resolve a
    // receipt-supplied path, which could point to an unrelated network share.
    let size = unsafe { GetShortPathNameW(root.as_ptr(), std::ptr::null_mut(), 0) };
    if size == 0 {
        return None;
    }
    let mut short = vec![0; size as usize];
    let written = unsafe { GetShortPathNameW(root.as_ptr(), short.as_mut_ptr(), size) };
    (written > 0 && written < size)
        .then(|| std::ffi::OsString::from_wide(&short[..written as usize]).into())
}

fn matches_directory_names(stored: &str, long: &str, short: &str) -> bool {
    let mut long = long.split('\\');
    let mut short = short.split('\\');
    stored
        .split('\\')
        .all(|component| match (long.next(), short.next()) {
            (Some(l), Some(s)) => component == l || component == s,
            _ => false,
        })
        && long.next().is_none()
        && short.next().is_none()
}

fn same_output_directory(root: &Path, stored: &Path) -> bool {
    let (Some(root_name), Some(stored_name)) =
        (directory_spelling(root), directory_spelling(stored))
    else {
        return false;
    };
    root_name == stored_name
        || short_directory_name(root)
            .and_then(|short| directory_spelling(&short))
            .is_some_and(|short| matches_directory_names(&stored_name, &root_name, &short))
}

/// A cached job may refer only to outputs in its own output directory. A
/// provider response containing similarly named properties is data, not a job.
fn state_output(root: &Path, raw: &Value, extension: &str) -> Result<PathBuf, String> {
    let path = Path::new(raw.as_str().ok_or("Job does not name an output artifact")?);
    let parent = path.parent().ok_or("Missing job output directory")?;
    if !parent.as_os_str().is_empty() && !same_output_directory(root, parent) {
        return Err("Job output is outside its local output directory".into());
    }
    let local = root.join(path.file_name().ok_or("Missing job output filename")?);
    if fs::symlink_metadata(&local)
        .map_err(|e| format!("Cannot inspect job output: {e}"))?
        .file_type()
        .is_symlink()
    {
        return Err("Job output is a symbolic link".into());
    }
    let path = local
        .canonicalize()
        .map_err(|e| format!("Cannot open job output: {e}"))?;
    if path.parent() != Some(root)
        || path.extension().and_then(|e| e.to_str()) != Some(extension)
        || !path.is_file()
    {
        return Err("Job references an unexpected output path".into());
    }
    Ok(path)
}

fn state_recognition(
    path: &Path,
    state: &Value,
    expected: &str,
    duration: Option<u64>,
) -> Result<Recognition, String> {
    let directory = path
        .parent()
        .and_then(Path::parent)
        .ok_or("Job has no output directory")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("Invalid job filename")?;
    let identity = stem.strip_suffix(".aggregate").unwrap_or(stem);
    if identity.len() != 64
        || !identity.bytes().all(|c| c.is_ascii_hexdigit())
        || state["identity"].as_str() != Some(identity)
    {
        return Err("Job identity does not match its filename".into());
    }
    if stem.ends_with(".aggregate") {
        let json = state_output(&directory, &state["json"], "json")?;
        let text = state_output(&directory, &state["text"], "txt")?;
        if text.file_stem() != json.file_stem()
            || !json
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.ends_with("-combined"))
        {
            return Err("Aggregate job text and JSON do not form an output pair".into());
        }
        let r = parse_aggregate(&read_json(&json)?, expected, duration)?;
        if r.text != read_text(&text)? {
            return Err("Aggregate text and recognition JSON disagree".into());
        }
        return Ok(r);
    }
    check_audio(state, expected)?;
    if state["version"] != 1 || state["status"] != "complete" {
        return Err("Selected recognition job is incomplete or unsupported".into());
    }
    let text_file = state_output(&directory, &state["text"], "txt")?;
    let text = read_text(&text_file)?;
    let offset = state["chunk_offset_frames"].as_u64();
    let frames = state["chunk_frames"].as_u64();
    if offset.is_some() != frames.is_some() || frames == Some(0) {
        return Err("Invalid job chunk frame bounds".into());
    }
    let offset_ms = frame_ms(offset.unwrap_or(0), 16_000).ok_or("Invalid job chunk offset")?;
    let chunk_end = offset
        .zip(frames)
        .map(|(o, f)| {
            o.checked_add(f)
                .and_then(|n| frame_ms(n, 16000))
                .ok_or("Invalid job chunk length")
        })
        .transpose()?;
    let limit = match (duration, chunk_end) {
        (Some(s), Some(e)) => Some(s.min(e)),
        (s, e) => s.or(e),
    }
    .and_then(|n| n.checked_sub(offset_ms));
    let mut recognition = if state["json"].is_null() {
        plain(text)
    } else {
        let json = state_output(&directory, &state["json"], "json")?;
        if json.file_stem() != text_file.file_stem() {
            return Err("Job text and JSON do not form an output pair".into());
        }
        let raw = read_json(&json)?;
        if raw["text"].as_str() != Some(text.as_str()) {
            let mut r = plain(text);
            r.issues.push(
                "Job text differs from provider JSON; timing and speakers are unavailable".into(),
            );
            r
        } else {
            let mut r = Recognition::default();
            Part {
                value: &raw,
                duration: limit,
                offset_ms,
                namespace: "",
            }
            .append(&mut r)?;
            r.render(true);
            r
        }
    };
    recognition.model = state["model"].as_str().map(str::to_owned);
    recognition.provider_profile = state["provider_profile"].as_str().map(str::to_owned);
    if let Some(end) = chunk_end {
        if offset_ms > 0 {
            recognition.gap(0, offset_ms);
        }
        if let Some(total) = duration {
            if total > end.saturating_add(100) {
                recognition.gap(end, total);
            }
        } else {
            recognition.issues.push(format!(
                "Only one chunk was selected ({offset_ms}..{end} ms)"
            ));
        }
    }
    Ok(recognition)
}

fn is_state(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == ".audio-capture-jobs")
}

// A generated output filename contains the first 16 hex digits of its job ID.
// The full receipt must also name the selected file. Similar text cannot prove
// that two artifacts came from the same source or chunk.
fn related_job(path: &Path) -> Result<Option<(PathBuf, Value)>, String> {
    let Some(root) = path.parent() else {
        return Ok(None);
    };
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let combined = stem.ends_with("-combined");
    let Some(short) = stem
        .rsplit('-')
        .find(|s| s.len() == 16 && s.bytes().all(|c| c.is_ascii_hexdigit()))
    else {
        return Ok(None);
    };
    let entries = match fs::read_dir(root.join(".audio-capture-jobs")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Cannot inspect recognition receipts: {e}")),
    };
    let mut found = None;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|n| n.strip_suffix(if combined { ".aggregate.json" } else { ".json" }))
        else {
            continue;
        };
        if id.len() != 64
            || !id.bytes().all(|c| c.is_ascii_hexdigit())
            || (!combined && !id.starts_with(short))
        {
            continue;
        }
        let state = read_json(&entry.path())?;
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let field = if ext.eq_ignore_ascii_case("txt") {
            "text"
        } else {
            "json"
        };
        if !state[field]
            .as_str()
            .is_some_and(|name| Path::new(name).file_name() == path.file_name())
        {
            continue;
        }
        if state_output(root, &state[field], ext)? != path {
            continue;
        }
        if state["identity"] != id || found.is_some() {
            return Err("Ambiguous or inconsistent recognition output receipt".into());
        }
        found = Some((entry.path(), state));
    }
    Ok(found)
}

/// Retrieve a verifiable audio association when the selected artifact records one.
/// Standalone text or untagged provider JSON carries no such association.
pub fn artifact_source(path: &Path) -> Result<Option<String>, String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let json = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("json"));
    let state = if is_state(&path) {
        Some((path.clone(), read_json(&path)?))
    } else {
        related_job(&path)?
    };
    let value = if let Some((job, state)) = state {
        if job
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.ends_with(".aggregate.json"))
        {
            let root = job
                .parent()
                .and_then(Path::parent)
                .ok_or("Invalid aggregate state location")?;
            read_json(&state_output(root, &state["json"], "json")?)?
        } else {
            state
        }
    } else if json {
        read_json(&path)?
    } else {
        return Ok(None);
    };
    Ok(value["source_sha256"].as_str().map(str::to_owned))
}

/// Load one explicitly selected canonical artifact. Job references are resolved
/// locally. Offsets in the returned recognition always refer to the saved audio.
pub fn load(
    path: &Path,
    expected_audio_sha256: &str,
    duration_ms: Option<u64>,
) -> Result<Recognition, String> {
    let path = path
        .canonicalize()
        .map_err(|e| format!("Cannot open recognition artifact: {e}"))?;
    if !path.is_file() {
        return Err("Recognition input must be a regular file".into());
    }
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if !is_state(&path)
        && let Some((receipt, state)) = related_job(&path)?
    {
        return state_recognition(&receipt, &state, expected_audio_sha256, duration_ms);
    }
    if extension.eq_ignore_ascii_case("txt") {
        return Ok(plain(read_text(&path)?));
    }
    if !extension.eq_ignore_ascii_case("json") {
        return Err("Select a UTF-8 .txt or .json recognition artifact".into());
    }
    let value = read_json(&path)?;
    if is_state(&path) {
        return state_recognition(&path, &value, expected_audio_sha256, duration_ms);
    }
    if value.get("parts").is_some() {
        return parse_aggregate(&value, expected_audio_sha256, duration_ms);
    }
    if value.get("source_sha256").is_some() {
        check_audio(&value, expected_audio_sha256)?;
    }
    parse_provider(&value, duration_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn speaker_prefixes_remain_separate_from_what_people_said() {
        let raw = json!({"text":"Agent: Morning.\n B: My name is Nia.","segments":[
            {"speaker":"agent","text":"Morning.","start":0.0,"end":1.0},
            {"speaker":"B","text":"My name is Nia.","start":1.1,"end":3.0}]});
        let r = parse_provider(&raw, Some(3000)).unwrap();
        assert_eq!(r.spans.len(), 4);
        assert_eq!(slice(&r, 0), "Agent: ");
        assert!(r.spans[0].labels.is_empty());
        assert_eq!(slice(&r, 2), "\n B: ");
        assert_eq!(slice(&r, 3), "My name is Nia.");
        assert_eq!(r.spans[3].labels, ["B"]);
        assert_eq!(r.spans[3].start_ms, Some(1100));
    }

    #[test]
    fn partial_speaker_projection_does_not_drop_other_times_or_invent_names() {
        let raw = json!({"text":"One. Two.","speaker_segments":[{"speaker":"s","text":"One.","start":0.0,"end":2.0}],
            "segments":[{"text":"One.","start":0.0,"end":2.0},{"text":"Two.","start":0.4,"end":1.0}]});
        let r = parse_provider(&raw, Some(3000)).unwrap();
        assert_eq!(r.spans.len(), 2);
        assert_eq!(r.spans[0].labels, ["s"]);
        assert_eq!(r.spans[1].start_ms, Some(400));
        assert!(r.spans[1].labels.is_empty());
    }

    #[test]
    fn receipt_directory_can_mix_known_long_and_short_components() {
        let long = r"c:\users\runneradmin\appdata\local\temp\recognition-test-output";
        let short = r"c:\users\runner~1\appdata\local\temp\recogn~1";
        let mixed = directory_spelling(Path::new(
            r"\\?\C:\Users\RUNNER~1\AppData\Local\Temp\recognition-test-output",
        ))
        .unwrap();
        assert!(matches_directory_names(&mixed, long, short));
        assert!(matches_directory_names(short, long, short));
        for outside in [
            r"c:\users\runner~2\appdata\local\temp\recognition-test-output",
            r"c:\users\runner~1\appdata\local\temp\recogn~1\child",
            r"c:\users\runner~1\appdata\local\temp",
            r"d:\users\runner~1\appdata\local\temp\recogn~1",
        ] {
            assert!(!matches_directory_names(outside, long, short));
        }
    }

    #[test]
    fn outside_receipt_cannot_select_a_local_file_with_the_same_name() {
        let local = std::env::current_exe().unwrap().canonicalize().unwrap();
        let root = local.parent().unwrap();
        let name = local.file_name().unwrap();
        assert_eq!(state_output(root, &json!(local), "exe").unwrap(), local);
        for outside in [
            root.join("unrelated"),
            root.join(".."),
            PathBuf::from(r"\\untrusted.invalid\share"),
        ] {
            let error = state_output(root, &json!(outside.join(name)), "exe").unwrap_err();
            assert_eq!(error, "Job output is outside its local output directory");
        }
    }

    #[test]
    fn job_output_alias_uses_original_source_digest_and_chunk_clock() {
        let key = "d".repeat(64);
        let scratch = std::env::temp_dir().join(format!(
            "recognition-inputs-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let jobs = scratch.join(".audio-capture-jobs");
        fs::create_dir_all(&jobs).unwrap();
        let text = scratch.join(format!("test-{}.txt", &key[..16]));
        let raw = text.with_extension("json");
        let state = jobs.join(format!("{key}.json"));
        fs::write(&text, "One.").unwrap();
        fs::write(
            &raw,
            json!({"text":"One.","segments":[{"text":"One.","speaker":"A","start":0,"end":0.5}]})
                .to_string(),
        )
        .unwrap();
        fs::write(&state,json!({"identity":key,"source_sha256":"abc","version":1,"status":"complete","json":raw,"text":text,
            "chunk_frames":16000,"chunk_offset_frames":32000,"provider_profile":"openai-diarized-v1"}).to_string()).unwrap();
        for path in [&raw, &state, &text] {
            assert_eq!(artifact_source(path).unwrap().as_deref(), Some("abc"));
            assert!(load(path, "def", Some(3000)).is_err());
            let r = load(path, "abc", Some(3000)).unwrap();
            assert_eq!(r.spans[0].start_ms, Some(2000));
            assert_eq!(r.gaps[0].0, [0, 2000]);
            assert_eq!(r.provider_profile.as_deref(), Some("openai-diarized-v1"));
        }
        fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn words_split_one_segment_preserving_unicode_and_separators() {
        let json = json!({"text":"  Hëllo,\t  世界!\n","segments":[{
            "start":0.0,"end":2.0,"text":"Hëllo, 世界!",
            "words":[{"start":0.0,"end":1.0,"word":" Hëllo,"},{"start":1.0,"end":2.0,"word":" 世界!"}]
        }]});
        let mut r = parse_provider(&json, Some(2000)).unwrap();
        align(
            &mut r,
            vec![interval("a", 0, 1000), interval("b", 1000, 2000)],
        );
        assert_eq!(r.spans.len(), 2);
        assert_eq!(r.spans[0].labels, ["a"]);
        assert_eq!(slice(&r, 1), "\t  世界!\n");
        assert_eq!(
            r.spans
                .iter()
                .map(|s| &r.text[s.bytes[0]..s.bytes[1]])
                .collect::<String>(),
            r.text
        );
    }

    #[test]
    fn word_groups_end_at_conversational_pauses_even_when_unassigned() {
        let raw = json!({"text":" A B.  C?","words":[
            {"word":"A","start":0.0,"end":0.2},
            {"word":"B.","start":0.3,"end":0.5},
            {"word":"C?","start":1.84,"end":2.1}]});
        let mut r = parse_provider(&raw, Some(2200)).unwrap();
        for activity in [
            Vec::new(),
            vec![interval("a", 0, 500), interval("a", 1840, 2100)],
        ] {
            align(&mut r, activity);
            assert_eq!(r.spans.len(), 2);
            assert_eq!((slice(&r, 0), r.spans[0].end_ms), (" A B.", Some(500)));
            assert_eq!((slice(&r, 1), r.spans[1].start_ms), ("  C?", Some(1840)));
        }
    }

    fn interval(label: &str, start_ms: u64, end_ms: u64) -> Interval {
        Interval {
            label: label.into(),
            start_ms,
            end_ms,
        }
    }

    fn slice(r: &Recognition, n: usize) -> &str {
        &r.text[r.spans[n].bytes[0]..r.spans[n].bytes[1]]
    }

    #[test]
    fn local_one_speaker_cannot_identify_two_competing_recognition_segments() {
        let raw = json!({"text":"We have a meeting at four. All right.","segments":[
            {"text":"We have a meeting at four.","start":1.0,"end":3.6,"speaker":"p1"},
            {"text":"All right.","start":2.5,"end":3.0,"speaker":"p2"}]});
        let mut r = parse_provider(&raw, Some(4000)).unwrap();
        assert_eq!(r.spans[0].labels, ["p1"]);
        assert_eq!(r.spans[1].labels, ["p2"]);
        align(&mut r, vec![interval("local", 900, 3800)]);
        assert!(r.spans.iter().all(|s| s.labels.is_empty()));
        assert_eq!(r.activity.len(), 1);
        assert_eq!(r.spans[1].start_ms, Some(2500));
        assert_eq!(slice(&r, 1), " All right.");
    }

    #[test]
    fn small_timing_jitter_and_repeated_recognition_do_not_hide_a_speaker() {
        for (text, start, end) in [("Check the list.", 1.85, 2.6), ("On it.", 1.25, 1.9)] {
            let raw = json!({"text":format!("On it. {text}"),"segments":[
                {"text":"On it.","start":1.0,"end":2.0},{"text":text,"start":start,"end":end}]});
            let mut r = parse_provider(&raw, Some(3000)).unwrap();
            align(&mut r, vec![interval("local", 900, 2700)]);
            assert!(r.spans.iter().all(|s| s.labels == ["local"]));
        }
    }

    #[test]
    fn coarse_or_simultaneous_speech_stays_ambiguous() {
        let json = json!({"text":"One turn.","segments":[{"start":0,"end":2,"text":"One turn."}]});
        let mut r = parse_provider(&json, Some(2000)).unwrap();
        for activity in [
            vec![interval("a", 0, 1000), interval("b", 1000, 2000)],
            vec![interval("a", 0, 2000), interval("b", 0, 2000)],
        ] {
            align(&mut r, activity);
            assert_eq!(r.spans.len(), 1);
            assert_eq!(r.spans[0].labels, ["a", "b"]);
            assert_eq!(r.activity.len(), 2);
        }
    }

    #[test]
    fn unmatched_text_and_invalid_or_unverified_times_survive() {
        let mut json = json!({"text":"  Two parts.  ","segments":[{"start":0,"end":1,"text":"Two","speaker":"a"}]});
        let r = parse_provider(&json, Some(2000)).unwrap();
        assert_eq!(slice(&r, 0), "  Two parts.  ");
        assert_eq!(r.spans[0].start_ms, None);
        assert_eq!(r.activity.len(), 1);
        json["segments"][0]["text"] = json!("Two parts.");
        for duration in [Some(800), None] {
            let r = parse_provider(&json, duration).unwrap();
            assert_eq!(r.spans[0].start_ms, None);
            assert!(r.activity.is_empty());
        }
    }

    #[test]
    fn aggregate_uses_frame_offsets_once_and_scopes_cluster_labels() {
        let part = |offset, text, speaker| {
            json!({"offset_frames":offset,"frames":16000,"sample_rate":16000,"text":text,
            "segments":[{"start":0.1,"end":0.8,"text":text}],
            "speaker_segments":[{"start":0.1,"end":0.8,"text":text,"speaker":speaker}]})
        };
        let raw = json!({"source_sha256":"abc","sample_rate":16000,"parts":[part(0,"First","A"),part(32000,"Second","A")]});
        assert!(parse_aggregate(&raw, "changed", Some(3000)).is_err());
        let r = parse_aggregate(&raw, "abc", Some(3000)).unwrap();
        assert_eq!(r.text, "First\nSecond");
        assert_eq!(r.spans.last().unwrap().start_ms, Some(2100));
        assert_eq!(r.spans.last().unwrap().end_ms, Some(2800));
        assert_ne!(r.activity[0].label, r.activity[1].label);
        assert!(
            r.issues
                .iter()
                .any(|issue| issue.contains("1000") && issue.contains("2000"))
        );
    }
}
