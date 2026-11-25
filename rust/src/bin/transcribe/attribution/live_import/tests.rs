use super::*;
use std::fs;
use std::io::Write;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(super::super::model::new_id("live_refinement_test").unwrap());
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn journal(&self) -> PathBuf {
        let path = self.path("Transcript-example.segments.jsonl");
        let mut rows = vec![
            json!({"type":"begin","schema":1,"session":"capture","interval":"text","start_frame":0,"sources":["App","Mic"]}),
        ];
        for (id, a, b, t) in [
            ("one", 0, 4800, "  Héllo"),
            ("pause", 4800, 9600, " skipped\n"),
            ("two", 9600, 14400, "there."),
            ("gap", 14400, 19200, "?"),
            ("after", 19200, 24000, " later"),
        ] {
            rows.push(
                json!({"type":"request","id":id,"source":"App","start_frame":a,"end_frame":b}),
            );
            rows.push(json!({"type":"speech","id":id,"source":"App","start_frame":a,"end_frame":b,"text":t,"segments":[{"text":t,"start_frame":a,"end_frame":b}]}));
        }
        rows.extend([
            json!({"type":"speaker_revision","source":"App","start_frame":0,"end_frame":4800,"revision":3,"update":{},"assignments":[{"id":"one","ordinal":0,"start_frame":0,"end_frame":4800,"epoch":1,"revision":3,"speaker":"Voice 1"}]}),
            json!({"type":"request","id":"raw","source":"Mic","start_frame":0,"end_frame":4800}),
            json!({"type":"speech","id":"raw","source":"Mic","start_frame":0,"end_frame":4800,"text":"Mic; no timing."}),
            json!({"type":"gap","id":"missing","source":"Mic","start_frame":4800,"end_frame":9600,"reason":"private diagnostic"}),
            json!({"type":"recording","id":"rec","start_frame":0,"end_frame":19200}),
            json!({"type":"end","cutoff_frame":24000,"incomplete":true,"speech_entries":6,"gaps":1})
        ]);
        for (i, row) in rows.iter_mut().enumerate() {
            row["sequence"] = json!(i + 1)
        }
        write_lines(&path, &rows);
        path
    }
    fn media(&self) -> Media {
        let media = self.path("app.wav");
        let samples = 14400u32;
        let bytes = samples * 2;
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36 + bytes).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(48000u32.to_le_bytes());
        wav.extend(96000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(bytes.to_le_bytes());
        wav.resize(44 + bytes as usize, 0);
        fs::write(&media, wav).unwrap();
        let (bytes, last_write, volume, file_index) =
            timeline::identity(&pinned(&media).unwrap()).unwrap();
        let map = self.path("Recording-rec.timeline.jsonl");
        write_lines(
            &map,
            &[
                json!({"type":"recording","v":1,"rate":48000,"session":"capture","recording":"rec","sources":["App","Mic"],"start":0,"mapped_from":0,"saved_at_mapped_from":0,"outputs":[{"file":"app.wav","source":0}],"format":"wav","seek_precision":"pcm-frame","output_rate":48000}),
                json!({"type":"span","kind":"saved","start":0,"end":4800,"saved":0}),
                json!({"type":"span","kind":"pause","start":4800,"end":9600,"saved":4800}),
                json!({"type":"span","kind":"saved","start":9600,"end":19200,"saved":4800}),
                json!({"type":"source_gap","source":0,"start":14400,"end":19200}),
                json!({"type":"transcript","interval":"text","start":0,"end":19200}),
                json!({"type":"file_identity","file":"app.wav","bytes":bytes,"last_write":last_write,"volume":volume,"file_index":file_index}),
                json!({"type":"pcm_part","file":"app.wav","first_sample":0,"samples":samples,"rate":48000}),
                json!({"type":"end","complete":true,"end":19200,"saved_frames":samples}),
            ],
        );
        let powershell = PathBuf::from(std::env::var_os("WINDIR").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let cfg = self.path("adapter.json");
        fs::write(&cfg,serde_json::to_vec(&json!({"program":powershell,"args":["-NoProfile","-NonInteractive","-Command",
            "$r=[Console]::In.ReadToEnd() | ConvertFrom-Json; if ($r.duration_ms -ne 300 -or !(Test-Path -LiteralPath $r.audio_path)){exit 1}; @{protocol_version=1;source_id=$r.source_id;backend='test';model='fixture';intervals=@(@{speaker='cluster';start_ms=0;end_ms=300})} | ConvertTo-Json -Depth 5 -Compress"]})).unwrap()).unwrap();
        (map, media, cfg)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn write_lines(path: &Path, rows: &[Value]) {
    let mut file = File::create(path).unwrap();
    for row in rows {
        serde_json::to_writer(&mut file, row).unwrap();
        file.write_all(b"\n").unwrap()
    }
}
fn turn<'a>(session: &'a Session, text: &str) -> &'a Turn {
    session.turns.iter().find(|t| t.raw_text == text).unwrap()
}
#[test]
fn preserves_original_review_and_source_clock_and_exports_only_approved_redaction() {
    let f = Fixture::new();
    let path = f.journal();
    let target = f.path("session.json");
    let initial = run(&path, &target, None).unwrap();
    assert_eq!(initial.revision, 1);
    assert_eq!(
        initial.sources[0].raw_text,
        "  Héllo skipped\nthere.? later"
    );
    assert_eq!(turn(&initial, "  Héllo").attribution, "provisional");
    assert_eq!(turn(&initial, "Mic; no timing.").timing_method, "chunk");
    assert!(!initial.complete);
    let patch = json!({"session_id":initial.session_id,"expected_revision":1,"operations":[{"op":"correct_text","turn_id":turn(&initial,"  Héllo").turn_id,"text":"Reviewed."}]});
    let reviewed = super::super::review(&target, 1, patch).unwrap();
    let choice = f.media();
    let refined = run(&path, &target, Some(&choice)).unwrap();
    assert_eq!(refined.revision, 3);
    let review = super::super::decisions::render(&refined);
    assert!(
        review.contains("capture session timeline")
            && review.contains("200 to 300 ms")
            && review.contains("Live source \"App\"")
    );
    assert_eq!(turn(&refined, "  Héllo"), turn(&reviewed, "  Héllo"));
    for raw in [" skipped\n", "?", " later", "Mic; no timing."] {
        assert_eq!(turn(&refined, raw), turn(&initial, raw))
    }
    let eligible = turn(&refined, "there.");
    assert_eq!(eligible.attribution, "resolved");
    assert_eq!(eligible.start_ms, Some(200));
    assert_eq!(
        refined
            .activities
            .iter()
            .map(|a| (a.start_ms, a.end_ms))
            .collect::<Vec<_>>(),
        vec![(0, 100), (200, 300)]
    );
    assert_eq!(refined.decisions, reviewed.decisions);
    assert_eq!(
        refined.sources[0].provenance["refinements"][0]["saved_coverage_frames"],
        json!([[0, 4800], [9600, 14400]])
    );
    assert_eq!(run(&path, &target, None).unwrap(), refined);
    assert_eq!(run(&path, &target, Some(&choice)).unwrap(), refined);
    let history = f
        .path(".audio-capture-sessions")
        .join(&initial.session_id)
        .join("00000000000000000001.json");
    let saved: Session = store::read_json(&history).unwrap();
    assert_eq!(saved, initial);
    let scope = f.path("scope.json");
    let stream = f.path("stream");
    fs::write(&scope,serde_json::to_vec(&json!({"schema_major":1,"sessions":[{"manifest":"session.json","revision":3,"turns":[eligible.turn_id],"redactions":{(eligible.turn_id.as_str()):"Allowed"}}]})).unwrap()).unwrap();
    export::run(&scope, &stream, None, false).unwrap();
    let receipt: Value =
        serde_json::from_str(&export::run(&scope, &stream, None, true).unwrap()).unwrap();
    let event_file = Path::new(receipt["batch"].as_str().unwrap()).with_file_name("events.jsonl");
    let events = fs::read_to_string(event_file).unwrap();
    assert!(events.contains("capture_session") && events.contains("Allowed"));
    for private in [
        "Héllo",
        "there.",
        "later",
        "diagnostic",
        "Reviewed.",
        "app.wav",
        "adapter.json",
    ] {
        assert!(!events.contains(private))
    }
}
#[test]
fn refuses_incomplete_index_wrong_source_mixed_output_and_changed_media() {
    let f = Fixture::new();
    let path = f.journal();
    let target = f.path("session.json");
    let choice = f.media();
    let good = fs::read_to_string(&choice.0).unwrap();
    for (original, replacement) in [
        ("\"source\":0}]", "\"source\":null}]"),
        ("[\"App\",\"Mic\"]", "[\"Mic\",\"App\"]"),
        ("\"complete\":true", "\"complete\":false"),
        ("\"interval\":\"text\"", "\"interval\":\"stranger\""),
    ] {
        assert!(good.contains(original));
        fs::write(&choice.0, good.replace(original, replacement)).unwrap();
        assert!(run(&path, &target, Some(&choice)).is_err());
        assert!(!target.exists());
    }
    fs::write(&choice.0, good).unwrap();
    OpenOptions::new()
        .append(true)
        .open(&choice.1)
        .unwrap()
        .write_all(b"x")
        .unwrap();
    assert!(
        run(&path, &target, Some(&choice))
            .unwrap_err()
            .contains("changed")
    );
    assert!(!target.exists());
    let unfinished = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .take(2)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&path, unfinished).unwrap();
    assert!(run(&path, &target, None).is_err());
}
#[test]
fn rollover_uses_part_origin_and_rejects_discontinuous_pcm_offsets() {
    let f = Fixture::new();
    let path = f.journal();
    let target = f.path("rollover.json");
    let (map, first, cfg) = f.media();
    let second = f.path("app_part2.wav");
    fs::rename(&first, &second).unwrap();
    let mut rows: Vec<Value> = fs::read_to_string(&map)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    for row in &mut rows {
        match row["type"].as_str().unwrap() {
            "span" if row["kind"] == "pause" => row["kind"] = json!("saved"),
            "span" if row["start"] == 9600 => row["saved"] = json!(9600),
            "file_identity" | "pcm_part" => row["file"] = json!("app_part2.wav"),
            "end" => row["saved_frames"] = json!(19200),
            _ => {}
        }
    }
    let i = rows.iter().position(|r| r["type"] == "pcm_part").unwrap();
    rows[i]["first_sample"] = json!(4800);
    rows.insert(
        i,
        json!({"type":"pcm_part","file":"app.wav","first_sample":0,"samples":4800,"rate":48000}),
    );
    write_lines(&map, &rows);
    let media = (map.clone(), second, cfg);
    let result = run(&path, &target, Some(&media)).unwrap();
    assert_eq!(turn(&result, "  Héllo").attribution, "provisional");
    assert_eq!(turn(&result, " skipped\n").attribution, "resolved");
    assert_eq!(turn(&result, "there.").attribution, "resolved");
    rows[i + 1]["first_sample"] = json!(4801);
    write_lines(&map, &rows);
    assert!(
        run(&path, &target, Some(&media))
            .unwrap_err()
            .contains("PCM part mapping")
    );
}
#[test]
fn new_model_run_replaces_previous_machine_activity_in_latest_revision() {
    let f = Fixture::new();
    let index = f.journal();
    let target = f.path("session.json");
    let choice = f.media();
    let before = run(&index, &target, Some(&choice)).unwrap();
    assert_eq!(before.revision, 2);
    let old_cfg = fs::read_to_string(&choice.2).unwrap();
    assert!(old_cfg.contains("model='fixture'"));
    fs::write(
        &choice.2,
        old_cfg.replace("model='fixture'", "model='updated'"),
    )
    .unwrap();
    let after = run(&index, &target, Some(&choice)).unwrap();
    assert_eq!(after.revision, 3);
    assert_eq!(after.activities.len(), before.activities.len());
    let passes = after.sources[0].provenance["refinements"]
        .as_array()
        .unwrap();
    assert_eq!(passes.len(), 1);
    assert_eq!(passes[0]["model"], "updated");
}
