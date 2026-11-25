//! Verify an explicit local media selection and convert saved-file time to capture time.
use super::super::adapter::{self, AdapterConfig};
use super::super::model::Activity;
use super::*;
use std::os::windows::io::AsRawHandle;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
};

#[derive(Clone, Copy)]
struct Span {
    start: u64,
    end: u64,
    saved: u64,
}
struct Map {
    source: usize,
    record: String,
    spans: Vec<Span>,
    overlap: Range,
    duration_ms: u64,
    origin: u64,
    gaps: Vec<Range>,
    precision: String,
}
pub(super) struct Refinement {
    source: String,
    key: String,
    metadata: Value,
    regions: Vec<(Range, String)>,
    coverage: Vec<Range>,
}
fn scale(value: u64, factor: u64, rate: u64) -> Result<u64> {
    if rate == 0 {
        return Err("Invalid sample rate.".into());
    }
    u64::try_from(u128::from(value) * u128::from(factor) / u128::from(rate))
        .map_err(|_| "Sample time overflow.".into())
}
fn matches_output(selected: &str, base: &str, wav: bool) -> bool {
    if selected == base {
        return true;
    }
    wav && base
        .strip_suffix(".wav")
        .and_then(|stem| selected.strip_prefix(stem))
        .and_then(|part| part.strip_prefix("_part"))
        .and_then(|part| part.strip_suffix(".wav"))
        .and_then(|num| num.parse::<u32>().ok())
        .is_some_and(|n| n >= 2)
}
pub(super) fn identity(file: &File) -> Result<(u64, u64, u64, u64)> {
    let mut i: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut i) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let pair = |hi: u32, lo: u32| (u64::from(hi) << 32) | u64::from(lo);
    Ok((
        pair(i.nFileSizeHigh, i.nFileSizeLow),
        pair(
            i.ftLastWriteTime.dwHighDateTime,
            i.ftLastWriteTime.dwLowDateTime,
        ),
        u64::from(i.dwVolumeSerialNumber),
        pair(i.nFileIndexHigh, i.nFileIndexLow),
    ))
}
fn read_map(index: &Index, timeline: &File, audio: &File, leaf: &str) -> Result<Map> {
    let mut map = Map {
        source: 0,
        record: String::new(),
        spans: vec![],
        overlap: (0, 0),
        duration_ms: 0,
        origin: 0,
        gaps: vec![],
        precision: String::new(),
    };
    let mut cursor = 0;
    let mut saved = 0;
    let mut first = true;
    let mut ended = false;
    let mut linked = false;
    let mut wav = false;
    let mut checked_file = false;
    let mut part = None;
    let mut output_rate = 0;
    let mut base = String::new();
    let mut pcm_offset = 0u64;
    let mut part_number = 1u64;
    lines(timeline, |v| {
        let kind = string(&v, "type")?;
        if ended || first != (kind == "recording") {
            return Err("Invalid recording timeline order.".into());
        }
        first = false;
        match kind {
            "recording" => {
                let labels: Vec<String> = serde_json::from_value(v["sources"].clone())
                    .map_err(|_| "Missing recording sources.")?;
                if number(&v, "v")? != 1
                    || number(&v, "rate")? != 48000
                    || string(&v, "session")? != index.capture
                    || labels != index.labels
                {
                    return Err(
                        "Timeline source list or capture session differs from the live index."
                            .into(),
                    );
                }
                map.record = string(&v, "recording")?.into();
                map.overlap = *index
                    .recordings
                    .get(&map.record)
                    .ok_or("The recording has no overlap with this live interval.")?;
                cursor = number(&v, "mapped_from")?;
                saved = number(&v, "saved_at_mapped_from")?;
                let recording_start = number(&v, "start")?;
                if cursor < recording_start
                    || saved > cursor - recording_start
                    || map.overlap.0 < cursor
                {
                    return Err("Recording overlap precedes its mapping.".into());
                }
                wav = string(&v, "format")?.eq_ignore_ascii_case("wav");
                map.precision = string(&v, "seek_precision")?.into();
                output_rate = number(&v, "output_rate")?;
                let mut matches = v["outputs"]
                    .as_array()
                    .ok_or("Missing recording output list.")?
                    .iter()
                    .filter(|o| {
                        o["file"]
                            .as_str()
                            .is_some_and(|base| matches_output(leaf, base, wav))
                    });
                let output = matches
                    .next()
                    .ok_or("Selected media is absent from this recording.")?;
                if matches.next().is_some() {
                    return Err("Selected recording stem is ambiguous.".into());
                }
                base = string(output, "file")?.to_owned();
                map.source = match output.get("source") {
                    Some(Value::Null) if labels.len() == 1 => 0,
                    Some(v) => v
                        .as_u64()
                        .and_then(|v| usize::try_from(v).ok())
                        .filter(|i| *i < labels.len())
                        .ok_or("Choose an individual source stem for a multi-source recording.")?,
                    None => return Err("Recording output lacks source identity.".into()),
                };
            }
            "span" => {
                let (a, b) = range(&v, "start", "end")?;
                if a != cursor || number(&v, "saved")? != saved {
                    return Err("Discontinuous recording timeline.".into());
                }
                match string(&v, "kind")? {
                    "saved" => {
                        map.spans.push(Span {
                            start: a,
                            end: b,
                            saved,
                        });
                        saved = saved
                            .checked_add(b - a)
                            .ok_or("Saved frame count overflow.")?
                    }
                    "silence" | "pause" => {}
                    _ => return Err("Unknown recording span.".into()),
                }
                cursor = b;
            }
            "transcript" if v["interval"] == index.interval => {
                if linked || range(&v, "start", "end")? != map.overlap {
                    return Err("Recording and live interval boundaries differ.".into());
                }
                linked = true;
            }
            "source_gap" if number(&v, "source")? == map.source as u64 => {
                map.gaps.push(range(&v, "start", "end")?)
            }
            "timing_uncertain" if number(&v, "source")? == map.source as u64 => {
                let at = number(&v, "observed_at")?;
                if (map.overlap.0..map.overlap.1).contains(&at) {
                    return Err("This source has unbounded timing uncertainty during the selected interval.".into());
                }
            }
            "pcm_part" if wav && matches_output(string(&v, "file")?, &base, true) => {
                let name = string(&v, "file")?;
                let expected_name = if part_number == 1 {
                    base.clone()
                } else {
                    format!(
                        "{}_part{part_number}.wav",
                        base.strip_suffix(".wav")
                            .ok_or("Invalid PCM file extension.")?
                    )
                };
                let samples = number(&v, "samples")?;
                if name != expected_name
                    || number(&v, "first_sample")? != pcm_offset
                    || number(&v, "rate")? != output_rate
                {
                    return Err("Invalid PCM part mapping.".into());
                }
                if name == leaf {
                    part = Some((pcm_offset, samples));
                }
                pcm_offset = pcm_offset
                    .checked_add(samples)
                    .ok_or("PCM sample count overflow.")?;
                part_number += 1;
            }
            "file_identity" if v["file"] == leaf => {
                let expected = (
                    number(&v, "bytes")?,
                    number(&v, "last_write")?,
                    number(&v, "volume")?,
                    number(&v, "file_index")?,
                );
                if checked_file || expected != identity(audio)? {
                    return Err("Selected media has changed since recording.".into());
                }
                checked_file = true;
            }
            "end" => {
                if v["complete"] != true
                    || number(&v, "end")? != cursor
                    || number(&v, "saved_frames")? != saved
                    || cursor < map.overlap.1
                {
                    return Err("Recording is incomplete or its totals differ.".into());
                }
                ended = true;
            }
            "transcript" | "source_gap" | "timing_uncertain" | "file_identity" | "pcm_part" => {}
            _ => return Err("Unknown recording timeline entry.".into()),
        }
        Ok(())
    })?;
    if !ended || !linked || !checked_file {
        return Err(
            "Refinement requires a finalized, linked recording with its original file identity."
                .into(),
        );
    }
    map.duration_ms = if wav {
        let (first, samples) = part.ok_or("Missing finalized PCM part.")?;
        if pcm_offset != scale(saved, output_rate, 48000)? {
            return Err("PCM parts differ from the saved recording duration.".into());
        }
        map.origin = scale(first, 48000, output_rate)?;
        scale(samples, 1000, output_rate)?
    } else {
        saved.div_ceil(48)
    };
    Ok(map)
}
fn clipped(range: Range, other: Range) -> Option<Range> {
    let (a, b) = (range.0.max(other.0), range.1.min(other.1));
    (a < b).then_some((a, b))
}
pub(super) fn prepare(index: &Index, choice: &Media) -> Result<Refinement> {
    let (timeline, audio, config) = choice;
    let mut timeline_file = pinned(timeline)?;
    let mut audio_file = pinned(audio)?;
    let config_file = pinned(config)?;
    let source_dir = |p: &Path| {
        std::path::absolute(p)
            .and_then(|p| p.parent().unwrap_or(Path::new(".")).canonicalize())
            .map_err(|e| e.to_string())
    };
    if source_dir(timeline)? != source_dir(audio)? {
        return Err("Select media beside its recording timeline.".into());
    }
    let leaf = audio
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or("Invalid recording filename.")?;
    let map_hash = hash(&mut timeline_file)?;
    let map = read_map(index, &timeline_file, &audio_file, leaf)?;
    if timeline.file_name().and_then(|f| f.to_str())
        != Some(&format!("Recording-{}.timeline.jsonl", map.record))
    {
        return Err("Select the published recording timeline.".into());
    }
    let config: AdapterConfig = serde_json::from_reader(BufReader::new(config_file))
        .map_err(|e| format!("Invalid local adapter configuration: {e}"))?;
    if !config.program.is_absolute() {
        return Err("Use an absolute local adapter executable path.".into());
    }
    export::local_destination(&config.program)?;
    let media_hash = hash(&mut audio_file)?;
    let sid = &index.session.sources[map.source].source_id;
    let output = adapter::run(&config, sid, audio, map.duration_ms)?;
    let signature = jobs::digest(
        serde_json::to_vec(&output)
            .map_err(|e| e.to_string())?
            .as_slice(),
    )?;
    let key = stable_id("refine", &[sid, &map_hash, &media_hash, &signature]);
    let part_end = map
        .origin
        .saturating_add(map.duration_ms.saturating_mul(48));
    let mut regions = vec![];
    let mut coverage = vec![];
    let mut gaps = map.gaps.clone();
    gaps.sort_unstable();
    let mut merged: Vec<Range> = Vec::new();
    for (start, end) in gaps {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end)
        } else {
            merged.push((start, end))
        }
    }
    let mut kept = Vec::new();
    for span in &map.spans {
        let Some((a, b)) = clipped((span.start, span.end), map.overlap) else {
            continue;
        };
        let to_saved = |t: u64| span.saved + (t - span.start);
        let from_saved = |t: u64| span.start + (t - span.saved);
        if let Some((x, y)) = clipped((to_saved(a), to_saved(b)), (map.origin, part_end)) {
            let mut cursor = from_saved(x);
            let end = from_saved(y);
            let i = merged.partition_point(|gap| gap.1 <= cursor);
            for &(ga, gb) in merged[i..].iter().take_while(|gap| gap.0 < end) {
                if cursor < ga {
                    kept.push(Span {
                        start: cursor,
                        end: ga,
                        saved: to_saved(cursor),
                    })
                }
                cursor = cursor.max(gb);
            }
            if cursor < end {
                kept.push(Span {
                    start: cursor,
                    end,
                    saved: to_saved(cursor),
                })
            }
        }
    }
    // Status rows can split a continuous saved interval. They do not remove audio.
    let mut continuous: Vec<Span> = Vec::new();
    for span in kept {
        if let Some(prev) = continuous.last_mut()
            && prev.end == span.start
            && prev.saved + (prev.end - prev.start) == span.saved
        {
            prev.end = span.end;
        } else {
            continuous.push(span);
        }
    }
    for span in &continuous {
        coverage.push((span.start, span.end));
    }
    for interval in &output.intervals {
        let sa = map
            .origin
            .saturating_add(interval.start_ms.saturating_mul(48));
        let sb = map
            .origin
            .saturating_add(interval.end_ms.saturating_mul(48));
        let start = continuous.partition_point(|s| s.saved + (s.end - s.start) <= sa);
        for span in &continuous[start..] {
            if span.saved >= sb {
                break;
            }
            if let Some((x, y)) =
                clipped((sa, sb), (span.saved, span.saved + (span.end - span.start)))
                && let Some(r) = clipped(
                    (span.start + (x - span.saved), span.start + (y - span.saved)),
                    map.overlap,
                )
            {
                regions.push((r, stable_id("spk", &[sid, &key, &interval.speaker])));
            }
        }
    }
    regions.sort_unstable();
    regions.dedup();
    let metadata = json!({
        "id": key,
        "recording": map.record,
        "sha256": media_hash,
        "timeline_sha256": map_hash,
        "backend": output.backend,
        "model": output.model,
        "seek_precision": map.precision,
        "overlap_frames": map.overlap,
        "saved_coverage_frames": coverage,
        "source_gaps": map.gaps
    });
    Ok(Refinement {
        source: sid.clone(),
        key,
        metadata,
        coverage,
        regions,
    })
}
impl Refinement {
    pub(super) fn apply(self, old: &Session, bounds: &BTreeMap<String, Range>) -> Result<Session> {
        let mut next = old.clone();
        let source = next
            .sources
            .iter_mut()
            .find(|s| s.source_id == self.source)
            .ok_or("Missing live source.")?;
        let history = source.provenance["refinements"]
            .as_array_mut()
            .ok_or("Missing live provenance.")?;
        if history.iter().any(|v| v["id"] == self.key) {
            return Ok(next);
        }
        // A new model run for the same file replaces its previous machine activities.
        // Historical revisions and reviewed entities remain available.
        let scope = history.iter().position(|entry| {
            entry["sha256"] == self.metadata["sha256"]
                && entry["timeline_sha256"] == self.metadata["timeline_sha256"]
        });
        let mut retired = BTreeSet::new();
        if let Some(i) = scope {
            let prior = history.remove(i);
            if let Some(ids) = prior["activity_ids"].as_array() {
                retired.extend(ids.iter().filter_map(Value::as_str).map(str::to_owned));
            }
        }
        let activity_ids = self
            .regions
            .iter()
            .map(|((a, b), sid)| {
                stable_id(
                    "activity",
                    &[&self.key, sid, &a.to_string(), &b.to_string()],
                )
            })
            .collect::<Vec<_>>();
        let mut metadata = self.metadata.clone();
        metadata["activity_ids"] = json!(activity_ids);
        history.push(metadata);
        // Reviewed assignments and edited or derived turns take precedence over later model runs.
        let mut protected = BTreeSet::new();
        for d in &next.decisions {
            for field in [
                "turn_id",
                "speaker_id",
                "from_speaker_id",
                "into_speaker_id",
                "new_speaker_id",
                "person_id",
                "previous_person_id",
            ] {
                if let Some(id) = d[field].as_str() {
                    protected.insert(id);
                }
            }
            for field in ["turn_ids", "speaker_ids", "successors", "activity_ids"] {
                if let Some(ids) = d[field].as_array() {
                    protected.extend(ids.iter().filter_map(Value::as_str))
                }
            }
        }
        let reviewed_speakers = next
            .speakers
            .iter()
            .filter(|s| {
                s.display_name.is_some()
                    || s.person_id.is_some()
                    || protected.contains(s.speaker_id.as_str())
            })
            .map(|s| s.speaker_id.clone())
            .collect::<BTreeSet<_>>();
        let protected: BTreeSet<String> = protected.into_iter().map(str::to_owned).collect();
        next.activities.retain(|a| {
            !retired.contains(&a.activity_id)
                || protected.contains(&a.activity_id)
                || reviewed_speakers.contains(&a.speaker_id)
        });
        let mut known = next
            .speakers
            .iter()
            .map(|s| s.speaker_id.clone())
            .collect::<BTreeSet<_>>();
        let mut maxima = Vec::with_capacity(self.regions.len());
        let mut high = 0;
        for (((a, b), sid), activity_id) in self.regions.iter().zip(activity_ids) {
            high = high.max(*b);
            maxima.push(high);
            speaker(&mut next, &mut known, sid.clone());
            next.activities.push(Activity {
                activity_id,
                source_id: self.source.clone(),
                speaker_id: sid.clone(),
                backend: self.metadata["backend"].as_str().unwrap_or_default().into(),
                start_ms: a / 48,
                end_ms: b.div_ceil(48),
            });
        }
        for turn in next.turns.iter_mut().filter(|t| {
            t.source_id == self.source
                && t.timing_method != "chunk"
                && t.reviewed_text.is_none()
                && t.predecessors.is_empty()
                && !protected.contains(&t.turn_id)
                && !t.speaker_ids.iter().any(|s| reviewed_speakers.contains(s))
        }) {
            let Some(&(a, b)) = bounds.get(&turn.turn_id) else {
                continue;
            };
            let i = self.coverage.partition_point(|v| v.1 <= a);
            if !self.coverage.get(i).is_some_and(|r| r.0 <= a && r.1 >= b) {
                continue;
            }
            let i = maxima.partition_point(|end| *end <= a);
            let mut labels = BTreeSet::new();
            let mut covers = false;
            for ((x, y), id) in self.regions[i..].iter().take_while(|((x, _), _)| *x < b) {
                if *y > a {
                    labels.insert(id.clone());
                    covers |= *x <= a && *y >= b;
                }
            }
            turn.speaker_ids = if covers && labels.len() == 1 {
                labels.into_iter().collect()
            } else {
                vec![]
            };
            turn.attribution = if turn.speaker_ids.is_empty() {
                "unresolved"
            } else {
                "resolved"
            }
            .into();
        }
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or("Session revision limit reached.")?;
        Ok(next)
    }
}
