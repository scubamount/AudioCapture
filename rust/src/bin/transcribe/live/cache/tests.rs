use super::*;
use std::io::Write;

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    out: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir()
            .join(crate::attribution::model::new_id("transcribe-live-test").unwrap());
        let root = base.join("cache");
        let out = base.join("published");
        fs::create_dir_all(&root).unwrap();
        Self { base, root, out }
    }
    fn new_cache(&self) -> LiveCache {
        LiveCache::create_in(
            &self.root,
            "call",
            "take",
            &self.out,
            48000,
            &["Discord".into(), "Mic".into()],
        )
        .unwrap()
    }
    fn open(&self) -> LiveCache {
        LiveCache::open(&self.root, "take").unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn partial_is_readable_during_capture_and_only_committed_data_is_published() {
    let f = Fixture::new();
    let mut c = f.new_cache();
    assert!(LiveCache::open(&f.root, "take").is_err());
    assert!(c.timing("Unapproved source", 48000).is_err());
    c.request("word1", "Discord", 48000, 96000).unwrap();
    c.speech("word1", "Discord", 48000, 96000, Some("A"), "Hi ðŸ™‚")
        .unwrap();
    c.speech("word1", "Discord", 48000, 96000, Some("A"), "Hi ðŸ™‚")
        .unwrap();
    c.speech("word2", "Mic", 48000, 96000, None, "Yeah")
        .unwrap();
    c.recording("audio-1", 60000, 80000).unwrap();
    let partial = fs::read_to_string(c.partial_path()).unwrap();
    assert_eq!(partial.matches("Hi ðŸ™‚").count(), 1);
    assert!(partial.contains("chunk 00:00:01.000") && partial.contains("speaker A"));
    let out = c.finish(96000, false).unwrap();
    assert!(!c.partial_path().exists());
    assert!(
        fs::read_to_string(&out)
            .unwrap()
            .ends_with("[Status] Complete\n")
    );
    let index = fs::read_to_string(out.with_extension("segments.jsonl")).unwrap();
    assert!(index.contains("audio-1") && index.contains("word2") && index.contains("cutoff_frame"));
    assert!(c.speech("late", "Mic", 96000, 96001, None, "late").is_err());
    drop(c);
    assert_eq!(f.open().finish(96000, true).unwrap(), out);
}

#[test]
fn recovery_replays_one_durable_record_after_torn_text_and_unknown_request() {
    let f = Fixture::new();
    let mut c = f.new_cache();
    c.request("first", "Mic", 48000, 49000).unwrap();
    let save = fs::read(c.dir().join(CHECKPOINT)).unwrap();
    c.speech("first", "Mic", 48000, 49000, None, "cafÃ©")
        .unwrap();
    let partial = c.partial_path();
    // Simulate a crash after journaling the result, while writing its UTF-8 text.
    let bytes = fs::read(&partial).unwrap();
    fs::write(&partial, &bytes[..bytes.len() - 2]).unwrap();
    fs::write(c.dir().join(CHECKPOINT), save).unwrap();
    drop(c);
    let mut c = f.open();
    assert_eq!(
        fs::read_to_string(c.partial_path())
            .unwrap()
            .matches("cafÃ©")
            .count(),
        1
    );
    c.request("second", "Mic", 49000, 50000).unwrap();
    drop(c);
    let mut c = f.open();
    let out = c.finish(50000, true).unwrap();
    let text = fs::read_to_string(out).unwrap();
    assert_eq!(text.matches("cafÃ©").count(), 1);
    assert!(
        text.contains("[Gap]")
            && text.contains("Unknown remote outcome")
            && text.contains("[Status] Incomplete")
    );
}

#[test]
fn torn_journal_tail_does_not_create_words_and_old_committed_text_is_verified() {
    let f = Fixture::new();
    let mut c = f.new_cache();
    c.speech("ok", "Mic", 48000, 50000, None, "one").unwrap();
    let journal = c.dir().join(JOURNAL);
    let length = fs::metadata(&journal).unwrap().len();
    File::options()
        .append(true)
        .open(&journal)
        .unwrap()
        .write_all(br#"{"sequence":3,"type":"speech","#)
        .unwrap();
    drop(c);
    let c = f.open();
    assert_eq!(fs::metadata(&journal).unwrap().len(), length);
    let partial = c.partial_path();
    drop(c);
    let text = fs::read_to_string(&partial).unwrap().replace("one", "two");
    fs::write(partial, text).unwrap();
    assert!(LiveCache::open(&f.root, "take").is_err());
}

#[test]
fn faults_and_collisions_leave_user_files_unchanged() {
    let f = Fixture::new();
    let mut c = f.new_cache();
    fs::write(&f.out, "an existing file").unwrap();
    c.gap("gap", "Mic", 48000, 50000, "capture overflow")
        .unwrap();
    assert!(c.finish(50000, true).is_err());
    assert!(c.partial_path().is_file());
    drop(c);
    let mut c = f.open();
    let dest = f.base.join("alternate");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("Transcript-take.txt"), "keep me").unwrap();
    c.redirect_publication(&dest).unwrap();
    let output = c.finish(50000, true).unwrap();
    assert_eq!(output.file_name().unwrap(), "Transcript-take-1.txt");
    assert_eq!(
        fs::read_to_string(dest.join("Transcript-take.txt")).unwrap(),
        "keep me"
    );
    let kept = c.dir().join("unrelated.txt");
    fs::write(&kept, "keep").unwrap();
    c.discard_owned().unwrap();
    assert!(kept.is_file() && output.is_file());
    assert_eq!(fs::read_to_string(&f.out).unwrap(), "an existing file");
}

#[test]
fn empty_and_gated_intervals_do_not_claim_failed_recognition() {
    let f = Fixture::new();
    let mut c = f.new_cache();
    c.skipped("quiet", "Mic", 48000, 50000).unwrap();
    let txt = fs::read_to_string(c.finish(50000, false).unwrap()).unwrap();
    assert!(txt.contains("No speech recognized") && !txt.contains("[Gap]"));
    let mut empty = LiveCache::create_in(&f.root, "call", "empty", &f.out, 50000, &[]).unwrap();
    assert!(
        fs::read_to_string(empty.finish(50000, false).unwrap())
            .unwrap()
            .contains("No speech recognized")
    );
}

#[test]
fn labels_and_provider_timings_remain_separate_from_raw_words() {
    let f = Fixture::new();
    let mut c = f.new_cache();
    c.speech_timed(
        "a",
        "Discord",
        48000,
        50000,
        "Hello",
        vec![TimedSpeech {
            start_frame: 48002,
            end_frame: 49000,
            speaker: Some("A".into()),
            text: "Hello".into(),
        }],
    )
    .unwrap();
    c.speech_timed(
        "b",
        "Discord",
        50000,
        60000,
        "world",
        vec![TimedSpeech {
            start_frame: 51000,
            end_frame: 59000,
            speaker: Some("B".into()),
            text: "different".into(),
        }],
    )
    .unwrap();
    c.speaker_revision(
        "Discord",
        50000,
        58000,
        1,
        &serde_json::json!({"turns":[{"start":50000,"end":58000,"speaker":"Local-2"}]}),
    )
    .unwrap();
    c.speaker_revision("Discord", 50000, 58000, 2, &serde_json::json!({"turns":[]}))
        .unwrap();
    c.timing("Discord", 58000).unwrap();
    let txt = fs::read_to_string(c.finish(60000, false).unwrap()).unwrap();
    assert!(
        txt.contains("timed")
            && txt.contains("speaker A")
            && txt.contains("world")
            && txt.contains("[Speaker update]")
    );
    assert!(txt.contains("No speaker assigned in this revision") && txt.contains("[Timing]"));
    assert!(!txt.contains("different") && !txt.contains("speaker B"));
}

#[test]
fn unsafe_interval_and_linked_state_are_rejected() {
    let f = Fixture::new();
    assert!(LiveCache::create_in(&f.root, "call", "../take", &f.out, 0, &[]).is_err());
    let c = f.new_cache();
    let other = f.base.join("copy-of-checkpoint");
    let saved = fs::read(c.dir().join(CHECKPOINT)).unwrap();
    fs::hard_link(c.dir().join(CHECKPOINT), &other).unwrap();
    drop(c);
    assert!(LiveCache::open(&f.root, "take").is_err());
    assert_eq!(fs::read(other).unwrap(), saved);
}

#[test]
fn recovered_cutoff_includes_observed_annotations_after_the_last_request() {
    let fixture = Fixture::new();
    let mut cache = fixture.new_cache();
    cache.timing("Mic", 50001).unwrap();
    cache.recording("recording", 50001, 51000).unwrap();
    drop(cache);
    let mut cache = fixture.open();
    let cutoff = cache.state.last_frame;
    assert_eq!(cutoff, 51000);
    let output = cache.finish(cutoff, true).unwrap();
    let rows = fs::read_to_string(output.with_extension("segments.jsonl")).unwrap();
    let last: serde_json::Value = serde_json::from_str(rows.lines().last().unwrap()).unwrap();
    assert_eq!(last["cutoff_frame"], 51000);
    assert_eq!(last["incomplete"], true);
}

#[test]
fn crash_after_publication_verifies_the_actual_files_before_cleanup() {
    let f = Fixture::new();
    let mut c = f.new_cache();
    c.speech("id", "Mic", 48000, 51000, None, "preserve")
        .unwrap();
    let output = c.finish(51000, false).unwrap();
    fs::write(&output, "replaced by user").unwrap();
    drop(c);
    assert!(f.open().finish(51000, false).is_err());
    assert_eq!(fs::read_to_string(output).unwrap(), "replaced by user");
}

#[test]
fn explicit_discard_can_remove_a_corrupt_recovery_without_touching_other_files() {
    let fixture = Fixture::new();
    let cache = fixture.new_cache();
    fs::write(cache.dir().join(CHECKPOINT), b"torn json").unwrap();
    // The live worker never spools PCM, even under this plausible filename.
    let foreign = cache.dir().join("request.wav");
    fs::write(&foreign, b"unrelated").unwrap();
    drop(cache);
    assert!(LiveCache::open(&fixture.root, "take").is_err());
    LiveCache::open_cache(&fixture.root, "take", false)
        .unwrap()
        .discard_owned()
        .unwrap();
    assert_eq!(fs::read(foreign).unwrap(), b"unrelated");
}

#[test]
fn local_revisions_link_each_timed_span_in_either_arrival_order() {
    let fixture = Fixture::new();
    let mut cache = fixture.new_cache();
    let local = |speaker: &str| serde_json::json!({"epoch":1,"turns":[{"start":48000,"end":54000,"speaker":speaker}]});
    let segment = |start, end, word: &str| TimedSpeech {
        start_frame: start,
        end_frame: end,
        speaker: None,
        text: word.into(),
    };
    cache
        .speaker_revision("Mic", 48000, 54000, 1, &local("A"))
        .unwrap();
    cache
        .speech_timed(
            "first",
            "Mic",
            48000,
            50000,
            "first word",
            vec![segment(48010, 49990, "first word")],
        )
        .unwrap();
    cache
        .speech_timed(
            "other",
            "Discord",
            48000,
            51000,
            "reply",
            vec![segment(48100, 48900, "reply")],
        )
        .unwrap();
    cache
        .speech_timed(
            "second",
            "Mic",
            50000,
            53000,
            "second word",
            vec![segment(50010, 52990, "second word")],
        )
        .unwrap();
    cache
        .speaker_revision("Mic", 48000, 54000, 2, &local("B"))
        .unwrap();
    let preview = cache.last_preview().unwrap();
    assert!(preview.contains("revision 2") && preview.contains("provisional B"));
    cache
        .speaker_revision(
            "Mic",
            48000,
            54000,
            3,
            &serde_json::json!({"epoch":1,"turns":[]}),
        )
        .unwrap();
    let txt = cache.finish(54000, false).unwrap();
    let contents = fs::read_to_string(&txt).unwrap();
    assert_eq!(contents.matches("first word").count(), 1);
    assert!(
        contents.contains("provisional speaker A")
            && contents.contains("span first/0")
            && contents.contains("unassigned")
    );
    let index = fs::read_to_string(txt.with_extension("segments.jsonl")).unwrap();
    let rows: Vec<serde_json::Value> = index
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let find = |revision| {
        rows.iter()
            .find(|v| v["type"] == "speaker_revision" && v["revision"] == revision)
            .unwrap()
    };
    let corrected = &find(2)["assignments"];
    assert_eq!(corrected.as_array().unwrap().len(), 2);
    assert_eq!(corrected[0]["id"], "first");
    assert_eq!(corrected[0]["ordinal"], 0);
    assert_eq!(corrected[0]["speaker"], "B");
    assert_eq!(
        find(3)["assignments"][1]["speaker"],
        serde_json::Value::Null
    );
    assert!(rows.iter().find(|v| v["id"] == "other").unwrap()["assignments"].is_null());
    assert_eq!(rows[0]["sources"], serde_json::json!(["Discord", "Mic"]));
}

#[test]
fn metadata_caps_reads_and_rejects_oversized_writes_before_replacement() {
    let f = Fixture::new();
    storage::write_json(&f.root, "test.json", &"old").unwrap();
    let long = "a".repeat(1_048_576);
    assert!(storage::write_json(&f.root, "test.json", &long).is_err());
    assert_eq!(
        storage::read_json::<String>(&f.root.join("test.json")).unwrap(),
        "old"
    );
    // A valid JSON value followed by whitespace must count its trailing bytes.
    fs::write(
        f.root.join("test.json"),
        format!("0{}", " ".repeat(1_048_576)),
    )
    .unwrap();
    assert!(storage::read_json::<u64>(&f.root.join("test.json")).is_err());
}

#[test]
fn transient_read_handle_during_checkpoint_replacement_keeps_the_session_live() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
    let fixture = Fixture::new();
    let mut cache = fixture.new_cache();
    let path = cache.dir().join(CHECKPOINT);
    let reader = File::options()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .unwrap();
    let probe = fixture.root.join("probe.tmp");
    fs::write(&probe, b"probe").unwrap();
    assert!(storage::rename(&probe, &path, true).is_err());
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(35));
        drop(reader);
    });
    let result = cache.speech("id", "Mic", 48000, 49000, None, "preserved");
    release.join().unwrap();
    result.unwrap();
    let txt = cache.finish(49000, false).unwrap();
    assert_eq!(
        fs::read_to_string(txt)
            .unwrap()
            .matches("preserved")
            .count(),
        1
    );
}

#[test]
fn durable_preview_is_utf8_bounded_and_excludes_failed_updates() {
    let fixture = Fixture::new();
    let mut c = fixture.new_cache();
    assert!(c.last_preview().is_none());
    c.speech("one", "Mic", 48000, 49000, None, &"語".repeat(700))
        .unwrap();
    let expected = c.last_preview().unwrap().to_owned();
    assert_eq!(expected.chars().count(), 512);
    assert!(!expected.contains('\n'));
    assert!(
        c.speech("two", "Mic", 48000, 49001, None, "uncommitted")
            .is_err()
    );
    assert_eq!(c.last_preview(), Some(expected.as_str()));
}
