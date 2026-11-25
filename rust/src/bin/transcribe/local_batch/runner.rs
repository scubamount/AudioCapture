//! Run approved local jobs with bounded PCM and durable, source-timed results.

use super::super::{jobs::digest, local_asr::LocalRecognizer, model_pack, video::PcmReader};
use super::storage::{
    DirIdentity, dir_identity, hold_dir, id, lock, open_input, publish, read, root, try_lock,
    valid_id, write,
};
use super::{Job, Progress, authorize_job, page};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const SAMPLE_RATE: usize = 16_000;
const WINDOW_FRAMES: usize = 30 * SAMPLE_RATE;
const FRAMES_PER_MS: usize = SAMPLE_RATE / 1_000;
const TARGET_SEEK_MS: u64 = 24_000;
const SEEK_RANGE_MS: std::ops::RangeInclusive<u64> = 16_000..=27_000;
const MAX_RECORD: usize = 512 * 1024;
const MAX_SEGMENT_TEXT: usize = 8 * 1024;
const MAX_PAGE_BYTES: usize = 128 * 1024;

#[derive(Serialize, Deserialize)]
struct Segment {
    start_ms: u64,
    end_ms: u64,
    text: String,
}

pub(super) fn artifacts(job: &Job) -> (PathBuf, PathBuf) {
    let destination = job.scope.output.join(&job.id);
    (
        destination.join("transcript.txt"),
        destination.join("transcript.json"),
    )
}

pub(super) fn has_publication(dir: &Path, job: &Job) -> bool {
    let receipt: DirIdentity = match read(&dir.join("publication.json")) {
        Ok(receipt) => receipt,
        Err(_) => return false,
    };
    let destination = job.scope.output.join(&job.id);
    let Ok((lease, actual)) = hold_dir(&destination) else {
        return false;
    };
    if actual != destination || !dir_identity(&lease).is_ok_and(|identity| identity == receipt) {
        return false;
    }
    let (text, json) = artifacts(job);
    [text, json]
        .iter()
        .all(|path| open_input(path).is_ok_and(|(_, actual, _)| actual == *path))
}

fn progress_path(dir: &Path) -> PathBuf {
    dir.join("progress.json")
}

fn journal_path(dir: &Path) -> PathBuf {
    dir.join("segments.jsonl")
}

fn cancelled(dir: &Path) -> bool {
    dir.join("cancel").is_file()
}

pub(crate) fn run_job(id: &str) -> Result<(), String> {
    if !valid_id(id, "batch") {
        return Err("Invalid local batch ID.".into());
    }
    let state = root()?;
    let dir = state.join("jobs").join(id);
    let job: Job = read(&dir.join("job.json"))?;
    if job.id != id {
        return Err("Local batch ID changed.".into());
    }
    let Some(_running) = try_lock(&dir.join("running.lock"))? else {
        return Ok(());
    };
    let mut progress: Progress = read(&progress_path(&dir))?;
    if progress.state != "queued" {
        return Ok(());
    }
    match execute(&state, &dir, &job, &mut progress) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _mutation = lock(&dir.join("mutate.lock"))?;
            // Recovery can finish an atomic publication after a progress write fails.
            if progress.state != "publishing" {
                progress.state = if cancelled(&dir) {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
                if progress.state == "failed" {
                    progress.error = Some(error);
                }
                write(&progress_path(&dir), &progress)?;
            }
            Ok(())
        }
    }
}

fn execute(state: &Path, dir: &Path, job: &Job, progress: &mut Progress) -> Result<(), String> {
    // Wait for the model slot without opening media or loading weights. The OS
    // closes both leases if the worker dies, including during inference.
    let _inference = loop {
        if cancelled(dir) {
            return set_cancelled(dir, progress);
        }
        if let Some(handle) = try_lock(&state.join("inference.lock"))? {
            break handle;
        }
        thread::sleep(Duration::from_millis(100));
    };
    {
        let _mutation = lock(&dir.join("mutate.lock"))?;
        if cancelled(dir) {
            return mark_cancelled(dir, progress);
        }
        authorize_job(job)?;
        progress.state = "running".into();
        write(&progress_path(dir), progress)?;
    }

    let (mut input, actual, identity) = open_input(&job.scope.input)?;
    if actual != job.scope.input
        || identity != job.scope.identity
        || digest(&mut input)? != job.scope.sha256
    {
        return Err("The approved input changed. Preview the file again.".into());
    }
    input.rewind().map_err(|e| e.to_string())?;
    let _output = hold_output(job)?;
    if job.scope.model_revision != model_pack::REVISION {
        return Err("The approved model revision changed. Preview the file again.".into());
    }
    let mut decoder = PcmReader::open(&input)?;
    if cancelled(dir) {
        return set_cancelled(dir, progress);
    }
    let pack = model_pack::verified()?;
    if cancelled(dir) {
        return set_cancelled(dir, progress);
    }
    let mut recognizer = LocalRecognizer::load(pack.path())?;
    let mut journal = File::create_new(journal_path(dir))
        .map_err(|error| format!("Cannot create the local transcript journal. {error}"))?;
    let mut windows = PcmWindows::new();
    loop {
        if cancelled(dir) {
            return set_cancelled(dir, progress);
        }
        let (samples, has_more) = windows
            .fill(&mut decoder)
            .map_err(|error| format!("Cannot read the embedded audio track. {error}"))?;
        if samples.is_empty() {
            break;
        }
        if cancelled(dir) {
            return set_cancelled(dir, progress);
        }
        let relative = recognizer.recognize(samples)?;
        let offset_ms = ms(progress.processed_frames);
        let window_ms = ms(samples.len() as u64);
        let mut segments = relative
            .into_iter()
            .filter_map(|segment| {
                absolute_segment(
                    segment.start_ms,
                    segment.end_ms,
                    segment.text,
                    offset_ms,
                    window_ms,
                )
            })
            .collect::<Vec<_>>();
        let advance = commit_window(&mut segments, offset_ms, samples.len(), has_more);
        let _mutation = lock(&dir.join("mutate.lock"))?;
        if cancelled(dir) {
            return mark_cancelled(dir, progress);
        }
        authorize_job(job)?;
        append_window(&mut journal, &segments)?;
        progress.processed_frames += advance as u64;
        progress.segment_count += segments.len() as u64;
        write(&progress_path(dir), progress)?;
        windows.advance(advance);
    }
    drop(recognizer);
    drop(pack);
    drop(decoder);
    drop(journal);

    let pending = job.scope.output.join(id("staged")?);
    fs::create_dir(&pending)
        .map_err(|error| format!("Cannot create the transcript staging folder. {error}"))?;
    let (lease, actual) = hold_dir(&pending)?;
    if pending != actual {
        return Err("The transcript staging folder changed.".into());
    }
    let mut cleanup = StagedOutput {
        path: pending,
        identity: dir_identity(&lease)?,
        lease: Some(lease),
        created_text: false,
        created_json: false,
    };
    render(job, dir, &mut cleanup, progress)?;

    let _mutation = lock(&dir.join("mutate.lock"))?;
    if cancelled(dir) {
        return mark_cancelled(dir, progress);
    }
    authorize_job(job)?;
    let _checked_output = hold_output(job)?;
    write(&dir.join("publication.json"), &cleanup.identity)?;
    progress.state = "publishing".into();
    write(&progress_path(dir), progress)?;
    drop(cleanup.lease.take());
    if let Err(error) = publish(&cleanup.path, &job.scope.output.join(&job.id), false) {
        if let Ok((lease, actual)) = hold_dir(&cleanup.path)
            && actual == cleanup.path
            && dir_identity(&lease).is_ok_and(|identity| identity == cleanup.identity)
        {
            cleanup.lease = Some(lease);
        }
        progress.state = "running".into();
        return Err(format!("Cannot publish the local transcript. {error}"));
    }
    progress.state = "completed".into();
    if let Err(error) = write(&progress_path(dir), progress) {
        progress.state = "publishing".into();
        return Err(error);
    }
    Ok(())
}

fn hold_output(job: &Job) -> Result<File, String> {
    let (output, actual) = hold_dir(&job.scope.output)?;
    if actual != job.scope.output || dir_identity(&output)? != job.scope.output_identity {
        return Err("The approved output changed. Preview the folder again.".into());
    }
    Ok(output)
}

struct StagedOutput {
    path: PathBuf,
    identity: DirIdentity,
    lease: Option<File>,
    created_text: bool,
    created_json: bool,
}

impl Drop for StagedOutput {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        if self.created_text {
            let _ = fs::remove_file(self.path.join("transcript.txt"));
        }
        if self.created_json {
            let _ = fs::remove_file(self.path.join("transcript.json"));
        }
        drop(lease);
        let _ = fs::remove_dir(&self.path);
    }
}

fn set_cancelled(dir: &Path, progress: &mut Progress) -> Result<(), String> {
    let _mutation = lock(&dir.join("mutate.lock"))?;
    mark_cancelled(dir, progress)
}

fn mark_cancelled(dir: &Path, progress: &mut Progress) -> Result<(), String> {
    progress.state = "cancelled".into();
    write(&progress_path(dir), progress)
}

fn ms(frames: u64) -> u64 {
    frames / FRAMES_PER_MS as u64
}

fn absolute_segment(
    start: u64,
    end: u64,
    text: String,
    offset: u64,
    window: u64,
) -> Option<Segment> {
    let text = text.trim();
    let start = start.min(window);
    let end = end.min(window);
    if text.is_empty() || start >= end || text.len() > MAX_SEGMENT_TEXT {
        return None;
    }
    Some(Segment {
        start_ms: offset + start,
        end_ms: offset + end,
        text: text.into(),
    })
}

// Reserve one sample of lookahead to distinguish a full final window from a
// full window whose trailing speech needs more context. The retained overlap
// and new input share one bounded buffer.
struct PcmWindows {
    bytes: Vec<u8>,
    samples: Vec<i16>,
}

impl PcmWindows {
    fn new() -> Self {
        Self {
            bytes: vec![0; (WINDOW_FRAMES + 1) * 2],
            samples: Vec::with_capacity(WINDOW_FRAMES + 1),
        }
    }

    fn fill(&mut self, source: &mut impl Read) -> io::Result<(&[i16], bool)> {
        let missing = WINDOW_FRAMES + 1 - self.samples.len();
        append_pcm(source, &mut self.bytes[..missing * 2], &mut self.samples)?;
        let has_more = self.samples.len() > WINDOW_FRAMES;
        Ok((
            &self.samples[..self.samples.len().min(WINDOW_FRAMES)],
            has_more,
        ))
    }

    fn advance(&mut self, committed: usize) {
        self.samples.copy_within(committed.., 0);
        self.samples.truncate(self.samples.len() - committed);
    }
}

// Publish only a complete timed prefix of a continuing window. Decode every
// deferred segment again from its original samples, with right-side context.
fn commit_window(segments: &mut Vec<Segment>, offset: u64, frames: usize, has_more: bool) -> usize {
    if !has_more {
        return frames;
    }
    let cut = segments
        .iter()
        .map(|segment| segment.end_ms - offset)
        .filter(|end| SEEK_RANGE_MS.contains(end))
        .min_by_key(|end| end.abs_diff(TARGET_SEEK_MS))
        .unwrap_or_else(|| {
            let target = offset + TARGET_SEEK_MS;
            // When the target falls in a gap, keep the trailing context even
            // if this window produced no text. Otherwise keep the crossing
            // segment intact and advance to its last available timestamp.
            segments
                .iter()
                .filter(|segment| segment.start_ms < target && segment.end_ms > target)
                .map(|segment| segment.end_ms - offset)
                .max()
                .unwrap_or(TARGET_SEEK_MS)
        });
    segments.retain(|segment| segment.end_ms <= offset + cut);
    cut as usize * FRAMES_PER_MS
}

fn append_pcm(
    source: &mut impl Read,
    bytes: &mut [u8],
    samples: &mut Vec<i16>,
) -> io::Result<usize> {
    let mut filled = 0;
    while filled < bytes.len() {
        match source.read(&mut bytes[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    if !filled.is_multiple_of(2) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "The last PCM sample is incomplete.",
        ));
    }
    samples.extend(
        bytes[..filled]
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]])),
    );
    Ok(filled / 2)
}

fn append_window(journal: &mut File, segments: &[Segment]) -> Result<(), String> {
    if segments.is_empty() {
        return Ok(());
    }
    let data = serde_json::to_vec(segments).map_err(|e| e.to_string())?;
    if data.len() >= MAX_RECORD {
        return Err("A recognition window exceeded the transcript journal limit.".into());
    }
    journal
        .write_all(&data)
        .and_then(|()| journal.write_all(b"\n"))
        .and_then(|()| journal.sync_data())
        .map_err(|error| format!("Cannot save the local transcript journal. {error}"))
}

// Read only the segment count committed in progress.json. A later journal
// write or an incomplete record left by a crash cannot enter the result.
fn visit_segments(
    dir: &Path,
    committed: u64,
    mut visit: impl FnMut(u64, Segment) -> Result<bool, String>,
) -> Result<(), String> {
    if committed == 0 {
        return Ok(());
    }
    let mut reader = BufReader::new(File::open(journal_path(dir)).map_err(|e| e.to_string())?);
    let mut buffer = Vec::new();
    let mut index = 0;
    while index < committed {
        buffer.clear();
        let bytes = (&mut reader)
            .take(MAX_RECORD as u64 + 1)
            .read_until(b'\n', &mut buffer)
            .map_err(|e| e.to_string())?;
        if bytes == 0 || bytes > MAX_RECORD || buffer.last() != Some(&b'\n') {
            return Err("A committed transcript journal record is missing or invalid.".into());
        }
        let window: Vec<Segment> = serde_json::from_slice(&buffer).map_err(|e| e.to_string())?;
        for segment in window {
            if index >= committed || !visit(index, segment)? {
                return Ok(());
            }
            index += 1;
        }
    }
    Ok(())
}

pub(super) fn fetch(job: &Job, request: &Value) -> Result<Value, String> {
    let dir = root()?.join("jobs").join(&job.id);
    let progress: Progress = read(&progress_path(&dir))?;
    let (offset, limit) = page(request).map_err(|error| error.1)?;
    let (offset, limit) = (offset as u64, limit as u64);
    if offset > progress.segment_count {
        return Err("The transcript offset exceeds committed coverage.".into());
    }
    let mut segments = Vec::new();
    let mut text = String::new();
    visit_segments(&dir, progress.segment_count, |index, segment| {
        if index < offset {
            return Ok(true);
        }
        if index - offset >= limit || text.len() + segment.text.len() + 1 > MAX_PAGE_BYTES {
            return Ok(false);
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&segment.text);
        segments.push(segment);
        Ok(true)
    })?;
    let next = offset + segments.len() as u64;
    let completed = progress.state == "completed";
    let (text_path, json_path) = artifacts(job);
    Ok(json!({
        "batch_id": job.id,
        "state": progress.state,
        "partial": !completed,
        "sample_rate": SAMPLE_RATE,
        "processed_frames": progress.processed_frames,
        "segment_count": progress.segment_count,
        "segments": segments,
        "text": text,
        "text_path": completed.then_some(text_path),
        "json_path": completed.then_some(json_path),
        "next_offset": (next < progress.segment_count).then_some(next),
    }))
}

fn render(
    job: &Job,
    dir: &Path,
    pending: &mut StagedOutput,
    progress: &Progress,
) -> Result<(), String> {
    let transcript =
        File::create_new(pending.path.join("transcript.txt")).map_err(|e| e.to_string())?;
    pending.created_text = true;
    let mut text = BufWriter::new(transcript);
    let structured =
        File::create_new(pending.path.join("transcript.json")).map_err(|e| e.to_string())?;
    pending.created_json = true;
    let mut json_output = BufWriter::new(structured);
    let metadata = serde_json::to_string(&json!({
        "schema_version": 1,
        "provider": "local",
        "model": model_pack::MODEL_NAME,
        "model_revision": job.scope.model_revision,
        "source": job.scope.input,
        "source_sha256": job.scope.sha256,
        "audio_extraction": "windows-media-foundation-first-audio-pcm16-mono-16000-v1",
        "transcription_windowing": "whisper-timed-boundary-overlap-v1",
        "sample_rate": SAMPLE_RATE,
        "source_frames": progress.processed_frames,
        "duration_ms": ms(progress.processed_frames),
        "partial": false,
    }))
    .map_err(|e| e.to_string())?;
    write!(
        json_output,
        "{},\"text\":\"",
        metadata.trim_end_matches('}')
    )
    .map_err(|e| e.to_string())?;
    visit_segments(dir, progress.segment_count, |index, segment| {
        if index > 0 {
            text.write_all(b"\n").map_err(|e| e.to_string())?;
            json_output.write_all(b"\\n").map_err(|e| e.to_string())?;
        }
        text.write_all(segment.text.as_bytes())
            .map_err(|e| e.to_string())?;
        let quoted = serde_json::to_string(&segment.text).map_err(|e| e.to_string())?;
        json_output
            .write_all(&quoted.as_bytes()[1..quoted.len() - 1])
            .map_err(|e| e.to_string())?;
        Ok(true)
    })?;
    json_output
        .write_all(b"\",\"segments\":[")
        .map_err(|e| e.to_string())?;
    visit_segments(dir, progress.segment_count, |index, segment| {
        if index > 0 {
            json_output.write_all(b",").map_err(|e| e.to_string())?;
        }
        // Keep seconds for the existing recognition reader and integer
        // milliseconds for clients that use the control API's time units.
        serde_json::to_writer(
            &mut json_output,
            &json!({
                "start": segment.start_ms as f64 / 1_000.0,
                "end": segment.end_ms as f64 / 1_000.0,
                "start_ms": segment.start_ms,
                "end_ms": segment.end_ms,
                "text": segment.text,
            }),
        )
        .map_err(|e| e.to_string())?;
        Ok(true)
    })?;
    json_output.write_all(b"]}\n").map_err(|e| e.to_string())?;
    for buffer in [&mut text, &mut json_output] {
        buffer
            .flush()
            .and_then(|()| buffer.get_ref().sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_source_clock_across_inference_windows() {
        let segment = absolute_segment(10, 1_700, " hello ".into(), 30_000, 1_200).unwrap();
        assert_eq!(
            (segment.start_ms, segment.end_ms, segment.text.as_str()),
            (30_010, 31_200, "hello")
        );
        assert!(absolute_segment(2_000, 3_000, "outside".into(), 30_000, 1_200).is_none());
    }

    #[test]
    fn reads_final_short_window_and_rejects_incomplete_pcm() {
        let mut samples = Vec::new();
        let mut buffer = [0; 6];
        assert_eq!(
            append_pcm(&mut [1, 0, 0, 128].as_slice(), &mut buffer, &mut samples).unwrap(),
            2
        );
        assert_eq!(samples, [1, i16::MIN]);
        assert_eq!(
            append_pcm(&mut [].as_slice(), &mut buffer, &mut samples).unwrap(),
            0
        );
        assert_eq!(samples, [1, i16::MIN]);
        assert!(append_pcm(&mut [1].as_slice(), &mut buffer, &mut samples).is_err());
    }

    #[test]
    fn redecodes_words_truncated_at_a_window_boundary() {
        const TOTAL_FRAMES: usize = 31_000 * FRAMES_PER_MS + 7;
        let sample_at = |frame| (frame % 16_003) as i16;
        let data: Vec<u8> = (0..TOTAL_FRAMES)
            .flat_map(|frame| sample_at(frame).to_le_bytes())
            .collect();
        let mut input = data.as_slice();
        let mut windows = PcmWindows::new();
        let mut committed = 0;
        let mut transcript = Vec::new();
        let words = [
            (18_000, 23_800, "intro"),
            (24_000, 27_000, "deferred"),
            (28_100, 30_200, "crossing"),
            (30_300, 30_800, "final"),
        ];
        loop {
            let (samples, has_more) = windows.fill(&mut input).unwrap();
            if samples.is_empty() {
                break;
            }
            assert_eq!(samples[0], sample_at(committed));
            assert_eq!(
                samples[samples.len() - 1],
                sample_at(committed + samples.len() - 1)
            );
            let offset = ms(committed as u64);
            let duration = ms(samples.len() as u64);
            // Simulate a recognizer that returns a truncated word at the edge
            // when it cannot see the word's full audio.
            let mut segments: Vec<_> = words
                .into_iter()
                .filter(|&(start, _, _)| start >= offset && start < offset + duration)
                .filter_map(|(start, end, text)| {
                    let partial = if end > offset + duration { "cro" } else { text };
                    absolute_segment(
                        start - offset,
                        end - offset,
                        partial.into(),
                        offset,
                        duration,
                    )
                })
                .collect();
            let advance = commit_window(&mut segments, offset, samples.len(), has_more);
            if committed == 0 {
                assert!(has_more);
                assert_eq!(advance, 23_800 * FRAMES_PER_MS);
                assert_eq!(segments.len(), 1);
            } else {
                assert!(!has_more);
            }
            transcript.extend(segments.into_iter().map(|s| (s.start_ms, s.end_ms, s.text)));
            committed += advance;
            windows.advance(advance);
        }
        assert_eq!(committed, TOTAL_FRAMES);
        assert_eq!(transcript, words.map(|(a, b, text)| (a, b, text.into())));
    }

    #[test]
    fn exact_final_window_is_committed_once() {
        let bytes = vec![0; WINDOW_FRAMES * 2];
        let mut input = bytes.as_slice();
        let mut windows = PcmWindows::new();
        let (samples, has_more) = windows.fill(&mut input).unwrap();
        assert_eq!(samples.len(), WINDOW_FRAMES);
        assert!(!has_more);
        let mut segments =
            vec![absolute_segment(25_000, 30_000, "last".into(), 0, 30_000).unwrap()];
        let advance = commit_window(&mut segments, 0, samples.len(), has_more);
        assert_eq!(advance, WINDOW_FRAMES);
        assert_eq!(segments[0].text, "last");
        windows.advance(advance);
        assert!(windows.fill(&mut input).unwrap().0.is_empty());
    }

    #[test]
    fn ignores_journal_tail_that_progress_has_not_committed() {
        let dir = std::env::temp_dir().join(id("transcript-test").unwrap());
        fs::create_dir(&dir).unwrap();
        let path = journal_path(&dir);
        let mut journal = File::create_new(&path).unwrap();
        let first = absolute_segment(0, 20, "first".into(), 0, 30_000).unwrap();
        let second = absolute_segment(0, 20, "second".into(), 30_000, 30_000).unwrap();
        append_window(&mut journal, &[first]).unwrap();
        append_window(&mut journal, &[second]).unwrap();
        journal.write_all(b"[unfinished").unwrap();
        drop(journal);

        let mut seen = Vec::new();
        visit_segments(&dir, 1, |_, segment| {
            seen.push(segment.text);
            Ok(true)
        })
        .unwrap();
        assert_eq!(seen, ["first"]);
        assert!(visit_segments(&dir, 3, |_, _| Ok(true)).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
