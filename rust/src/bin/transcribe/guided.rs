use crate::{
    attribution::{
        adapter::AdapterConfig,
        model::{Manifest, SourceInput, new_id},
    },
    batch_queue, jobs, transport,
};
use audio_capture::is_native_video;
use audio_capture::transcription_config::{
    GUI_CONFIG_ENV, GUI_PID_ENV, TranscriptionConfig, is_gui_environment_variable,
};
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run(args: impl Iterator<Item = OsString>) -> Result<(), String> {
    run_with_speakers(args, None)
}

pub(super) fn run_with_speakers(
    args: impl Iterator<Item = OsString>,
    speaker_path: Option<&Path>,
) -> Result<(), String> {
    batch_queue::bind_gui()?;
    let (files, separate_clocks, duplicates) = batch_queue::expand(selected_files(args)?)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let snapshot = if std::env::var_os(GUI_PID_ENV).is_some() {
        std::env::var_os(GUI_CONFIG_ENV)
    } else {
        None
    };
    let config = load_config(&exe, snapshot.as_deref())?;
    let optional_path = if speaker_path.is_none() && io::stdin().is_terminal() {
        print!("Local speaker session manifest (optional, Enter to skip): ");
        io::stdout().flush().map_err(|e| e.to_string())?;
        let reply = read_answer(&mut io::stdin().lock()).map_err(|e| e.to_string())?;
        (!reply.is_empty()).then(|| PathBuf::from(reply.trim_matches('"')))
    } else {
        None
    };
    let speaker_path = speaker_path.or(optional_path.as_deref());
    let mut speaker = None;
    let mut entries = Vec::new();
    let selection = configure_checked(
        files,
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        separate_clocks,
        duplicates,
        config,
        |options, output| {
            entries = batch_queue::snapshot(options);
            if let Some(path) = speaker_path {
                speaker =
                    Some(batch_queue::preview_speaker(path, output).map_err(io::Error::other)?);
            }
            batch_queue::print_issues(&entries, output)
        },
    );
    match selection {
        Ok(Some(options)) => batch_queue::submit(options, entries, speaker),
        Ok(None) => {
            println!("Cancelled.");
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            println!("Cancelled.");
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn load_config(
    exe: &Path,
    snapshot: Option<&OsStr>,
) -> Result<Option<TranscriptionConfig>, String> {
    match snapshot {
        Some(value) if value.is_empty() => Ok(None),
        Some(value) => {
            let snapshot = value
                .to_str()
                .ok_or("Recorder transcription config is not valid Unicode.")?;
            TranscriptionConfig::from_gui_snapshot(snapshot).map(Some)
        }
        None => TranscriptionConfig::load_optional_next_to(exe),
    }
}

fn selected_files(mut args: impl Iterator<Item = OsString>) -> Result<Vec<PathBuf>, String> {
    if !args.next().is_some_and(|arg| arg == "--") {
        return Err("Use --guided -- FILE... without other options.".into());
    }
    let files: Vec<_> = args.map(PathBuf::from).collect();
    if files.is_empty() || files.iter().any(|file| file.as_os_str().is_empty()) {
        return Err("Select at least one file and supply a path for each file.".into());
    }
    Ok(files)
}

pub(super) fn read_answer(input: &mut impl BufRead) -> io::Result<String> {
    // This also bounds allocation when scripts redirect input to guided mode.
    const LIMIT: u64 = 128 * 1024;
    let mut line = String::new();
    let length = input.take(LIMIT + 1).read_line(&mut line)?;
    if length as u64 > LIMIT {
        return Err(io::Error::other("The answer is too long."));
    }
    if !line.ends_with('\n') {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    let line = line.strip_suffix('\n').unwrap();
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.chars().any(char::is_control) {
        return Err(io::Error::other("The answer contains control characters."));
    }
    Ok(line.trim().to_owned())
}

struct Prompts<'a, R, W> {
    input: &'a mut R,
    output: &'a mut W,
}

impl<R: BufRead, W: Write> Prompts<'_, R, W> {
    fn ask(&mut self, label: &str) -> io::Result<String> {
        write!(self.output, "{label} ")?;
        self.output.flush()?;
        read_answer(self.input)
    }

    fn confirm(&mut self, label: &str, default: bool) -> io::Result<bool> {
        let label = format!("{label} {}", if default { "[Y/n]" } else { "[y/N]" });
        loop {
            match self.ask(&label)?.to_ascii_lowercase().as_str() {
                "" => return Ok(default),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => writeln!(self.output, "Enter y or n.")?,
            }
        }
    }
}

#[cfg(test)]
fn configure(
    files: Vec<PathBuf>,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<Option<jobs::Options>> {
    configure_checked(files, input, output, false, false, None, |_, _| Ok(()))
}

fn ask_config(
    ui: &mut Prompts<'_, impl BufRead, impl Write>,
    default_dir: &Path,
) -> io::Result<Option<(TranscriptionConfig, String)>> {
    writeln!(ui.output, "No transcription config is loaded.")?;
    writeln!(
        ui.output,
        "Use Load config in Transcription settings, or enter the service settings below."
    )?;
    let (base_url, destination) = loop {
        let base =
            ui.ask("API base URL (HTTPS or loopback HTTP, no credentials, Enter to cancel):")?;
        if base.is_empty() {
            return Ok(None);
        }
        match transport::endpoint(&base, false) {
            Ok(endpoint) => break (base, transport::canonical_key(&endpoint)),
            Err(error) => writeln!(ui.output, "{error}")?,
        }
    };
    let model = loop {
        let model = ui.ask("Model name:")?;
        if !model.is_empty() {
            break model;
        }
        writeln!(ui.output, "Enter the model name your endpoint accepts.")?;
    };
    let key_env = loop {
        let name =
            ui.ask("API key environment variable NAME (blank for none, never enter a key):")?;
        if is_gui_environment_variable(OsStr::new(&name)) {
            writeln!(
                ui.output,
                "Choose a variable name that the recorder does not reserve."
            )?;
            continue;
        }
        if name
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
        {
            break name;
        }
        writeln!(
            ui.output,
            "Use letters, digits, and underscores. Start with a letter or underscore."
        )?;
    };
    let dir = ui.ask(&format!("Output directory [{}]:", default_dir.display()))?;
    let output_dir = if dir.is_empty() {
        default_dir.to_path_buf()
    } else {
        dir.into()
    };
    Ok(Some((
        TranscriptionConfig {
            endpoint: base_url,
            model,
            api_key_env: key_env,
            transcript_output_folder: output_dir,
            ..Default::default()
        },
        destination,
    )))
}

fn configure_checked<W: Write>(
    files: Vec<PathBuf>,
    input: &mut impl BufRead,
    output: &mut W,
    separate_clocks: bool,
    selected_duplicates: bool,
    config: Option<TranscriptionConfig>,
    before_confirmation: impl FnOnce(&jobs::Options, &mut W) -> io::Result<()>,
) -> io::Result<Option<jobs::Options>> {
    let mut ui = Prompts { input, output };
    writeln!(
        ui.output,
        "Transcribe selected files. Press Ctrl+C to cancel."
    )?;
    let default_dir = files[0].with_file_name("transcripts");
    let configured = config.is_some();
    let (config, destination) = match config {
        Some(config) => {
            let endpoint =
                transport::endpoint(&config.endpoint, false).map_err(io::Error::other)?;
            writeln!(ui.output, "Using transcription config.")?;
            (config, transport::canonical_key(&endpoint))
        }
        None => {
            let Some(settings) = ask_config(&mut ui, &default_dir)? else {
                return Ok(None);
            };
            settings
        }
    };
    let TranscriptionConfig {
        endpoint: base_url,
        model,
        api_key_env,
        transcript_output_folder,
        request_segment_timestamps,
        ..
    } = config;
    let key_env = (!api_key_env.is_empty()).then_some(api_key_env);
    let output_dir = if transcript_output_folder.as_os_str().is_empty() {
        default_dir
    } else {
        transcript_output_folder
    };
    let same_session = !separate_clocks
        && files.len() > 1
        && ui.confirm("Do these files share one recording timeline?", false)?;
    let timestamps = if configured {
        request_segment_timestamps
    } else {
        ui.confirm("Request timestamps?", false)?
    };
    let has_audio_files = files.iter().any(|file| !is_native_video(file));
    let chunk_long =
        has_audio_files && ui.confirm("Split audio files over 25 MiB with FFmpeg?", false)?;
    let review = if ui.confirm("Create a local HTML review?", true)? {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        Some(output_dir.join(format!("review-{time}-{}.html", std::process::id())))
    } else {
        None
    };

    writeln!(ui.output, "\nSelected files:")?;
    let mut has_video = false;
    for file in &files {
        if is_native_video(file) {
            has_video = true;
            writeln!(ui.output, "  {} (extract audio locally)", file.display())?;
        } else {
            writeln!(ui.output, "  {}", file.display())?;
        }
    }
    if has_video {
        writeln!(
            ui.output,
            "Video files use the first audio stream. Only WAV audio goes to the endpoint."
        )?;
    }
    writeln!(
        ui.output,
        "Send to: {destination}\nModel: {model}\nOutput directory: {}",
        output_dir.display()
    )?;
    let timeline = if same_session { "shared" } else { "separate" };
    let yes_no = |enabled| if enabled { "yes" } else { "no" };
    writeln!(ui.output, "Timelines: {timeline}")?;
    writeln!(ui.output, "Timestamps: {}", yes_no(timestamps))?;
    if has_audio_files {
        writeln!(
            ui.output,
            "Split oversized audio files: {}",
            yes_no(chunk_long)
        )?;
    }
    writeln!(
        ui.output,
        "API key configured: {}",
        yes_no(key_env.is_some())
    )?;
    if let Some(path) = &review {
        writeln!(ui.output, "Local HTML review: {}", path.display())?;
    }
    let mut options = jobs::Options {
        files,
        base_url,
        model,
        key_env,
        output_dir,
        same_session,
        timestamps,
        chunk_long,
        review,
        allow_duplicate_sources: selected_duplicates,
        ..Default::default()
    };
    if !selected_duplicates && jobs::duplicate_coverage(&options.files) {
        options.allow_duplicate_sources = ui.confirm(
            "The selected mix repeats audio from the separate tracks. Include both?",
            false,
        )?;
        if !options.allow_duplicate_sources {
            return Ok(None);
        }
    }
    before_confirmation(&options, ui.output)?;
    if !ui.confirm("Send the selected recordings for transcription?", false)? {
        return Ok(None);
    }
    Ok(Some(options))
}

pub fn prepare_session(
    path: &Path,
    files: Vec<PathBuf>,
    adapter: Option<AdapterConfig>,
    automatic_names: bool,
    naming_context: Value,
) -> Result<(), String> {
    if path.exists() {
        return Err(format!(
            "Session manifest already exists: {}",
            path.display()
        ));
    }
    let configured = configure_session(files, &mut io::stdin().lock(), &mut io::stdout().lock());
    let mut manifest = match configured {
        Ok(Some(manifest)) => manifest,
        Ok(None) => {
            println!("Cancelled.");
            return Ok(());
        }
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            println!("Cancelled.");
            return Ok(());
        }
        Err(error) => return Err(error.to_string()),
    };
    manifest.adapter = adapter;
    manifest.automatic_names = automatic_names;
    manifest.naming_context = naming_context;
    let contents = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    let path = std::path::absolute(path).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    }
    jobs::stage_and_publish_new(&path, &contents)
        .map_err(|e| format!("Cannot publish session manifest {}: {e}", path.display()))?;
    println!("Session manifest saved at {}", path.display());
    Ok(())
}

fn selected_local_file(path: PathBuf, valid: fn(&Path) -> bool) -> io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    if !valid(&path) || !fs::metadata(&path)?.is_file() {
        return Err(io::Error::other(format!(
            "Unsupported file: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn recognition_artifact(path: &Path) -> bool {
    path.extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json") || ext.eq_ignore_ascii_case("txt"))
}

fn configure_session(
    mut files: Vec<PathBuf>,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<Option<Manifest>> {
    let mut ui = Prompts { input, output };
    writeln!(
        ui.output,
        "Create a session for completed recordings and local transcripts."
    )?;
    if files.is_empty() {
        loop {
            let answer = ui.ask("Audio path (blank when done):")?;
            if answer.is_empty() {
                break;
            }
            files.push(answer.into());
        }
        if files.is_empty() {
            return Ok(None);
        }
    }
    let mut sources = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for file in files {
        let audio = selected_local_file(file, jobs::is_media)?;
        if !seen.insert(audio.clone()) {
            return Err(io::Error::other(format!(
                "Audio was selected twice: {}",
                audio.display()
            )));
        }
        writeln!(ui.output, "Audio file {}", audio.display())?;
        let artifact = loop {
            let answer = ui.ask("Local recognition artifact (.json or .txt):")?;
            match selected_local_file(answer.into(), recognition_artifact) {
                Ok(path) => break path,
                Err(error) => writeln!(ui.output, "{error}")?,
            }
        };
        let role = loop {
            let role = ui
                .ask("Source role [program/mic/system/mix/other] (default other):")?
                .to_ascii_lowercase();
            match role.as_str() {
                "" => break String::from("other"),
                "program" | "mic" | "system" | "mix" | "other" => break role,
                _ => writeln!(ui.output, "Enter program, mic, system, mix, or other.")?,
            }
        };
        sources.push(SourceInput {
            source_id: new_id("source").map_err(io::Error::other)?,
            role,
            audio,
            artifact,
            ..Default::default()
        });
    }

    let shared_clock = sources.len() == 1
        || ui.confirm(
            "Do the source files share the same recording start and timeline?",
            false,
        )?;
    let mixes = sources.iter().filter(|source| source.role == "mix").count();
    let overlapping_roles = sources.iter().any(|source| source.role == "system")
        && sources.iter().any(|source| source.role == "program");
    if (mixes > 0 && sources.len() > 1) || (shared_clock && overlapping_roles) {
        writeln!(
            ui.output,
            "Choose one representation for any audio that occurs in multiple files."
        )?;
        loop {
            for source in &mut sources {
                source.selected = Some(ui.confirm(
                    &format!("Include {} ({})?", source.audio.display(), source.role),
                    false,
                )?);
            }
            let mut selected = sources
                .iter()
                .filter(|source| source.selected == Some(true))
                .peekable();
            if selected.peek().is_none() {
                writeln!(ui.output, "Select at least one source.")?;
                continue;
            }
            let roles = selected
                .map(|source| source.role.as_str())
                .collect::<Vec<_>>();
            if (roles.contains(&"mix") && roles.len() > 1)
                || (shared_clock && roles.contains(&"system") && roles.contains(&"program"))
            {
                writeln!(
                    ui.output,
                    "Choose a mix by itself. For a shared timeline, select either system audio or program audio."
                )?;
                continue;
            }
            break;
        }
    }
    let representation = if sources.iter().any(|source| source.selected.is_some()) {
        "explicit"
    } else {
        "auto"
    };
    writeln!(
        ui.output,
        "Timeline: {}",
        if shared_clock { "shared" } else { "separate" }
    )?;
    writeln!(ui.output, "Sources for this session:")?;
    for source in &sources {
        let included = if source.selected == Some(false) {
            "alternate"
        } else {
            "selected"
        };
        writeln!(
            ui.output,
            "  {included} {} ({})",
            source.audio.display(),
            source.role
        )?;
    }
    if !ui.confirm("Save this local session manifest?", false)? {
        return Ok(None);
    }
    Ok(Some(Manifest {
        session_id: new_id("session").map_err(io::Error::other)?,
        shared_clock,
        representation: representation.into(),
        sources,
        ..Default::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_literal_paths_after_separator() {
        let args = [
            "--",
            "--timestamps",
            "folder with spaces\\microphone.opus",
            "--",
        ];
        let files = selected_files(args.iter().map(OsString::from)).unwrap();
        assert_eq!(
            files,
            args[1..].iter().map(PathBuf::from).collect::<Vec<_>>()
        );
        for invalid in [vec![], vec!["--"], vec!["--timestamps", "--", "one.wav"]] {
            assert!(selected_files(invalid.into_iter().map(OsString::from)).is_err());
        }
    }

    #[test]
    fn single_file_defaults_require_final_consent() {
        let file = PathBuf::from(r"C:\Audio\--日本語.opus");
        let answers = "http://localhost:1234/v1\nasr\n\n\n\n\n\n";
        for consent in ["\n", "n\n", "", "yes"] {
            let input = format!("{answers}{consent}");
            let mut log = Vec::new();
            let result = configure(vec![file.clone()], &mut input.as_bytes(), &mut log);
            if consent.ends_with('\n') {
                assert!(result.unwrap().is_none());
            } else {
                assert_eq!(result.err().unwrap().kind(), io::ErrorKind::UnexpectedEof);
            }
            let text = String::from_utf8(log).unwrap();
            assert!(text.contains(r"C:\Audio\--日本語.opus"));
            assert!(text.contains("http://localhost:1234/v1/audio/transcriptions"));
            assert!(text.contains("Timelines: separate\nTimestamps: no\nSplit oversized audio files: no\nAPI key configured: no\n"));
            assert!(!text.contains("share one recording timeline"));
        }
        let input = format!("{answers}y\n");
        let options = configure(vec![file], &mut input.as_bytes(), &mut Vec::new())
            .unwrap()
            .unwrap();
        assert_eq!(options.output_dir, PathBuf::from(r"C:\Audio\transcripts"));
        assert_eq!(
            options.review.unwrap().parent(),
            Some(options.output_dir.as_path())
        );
        assert!(options.key_env.is_none());
        assert!(!options.same_session && !options.timestamps && !options.chunk_long);
    }

    #[test]
    fn blank_endpoint_cancels_manual_setup_after_one_retry() {
        let mut answers = &b"example.org/v1\n  \r\nhttp://localhost:1234/v1\n"[..];
        let mut log = Vec::new();
        let result = configure(vec![PathBuf::from("recording.mp4")], &mut answers, &mut log);

        assert!(result.unwrap().is_none());
        assert_eq!(answers, b"http://localhost:1234/v1\n");
        let text = String::from_utf8(log).unwrap();
        assert!(text.contains("No transcription config is loaded."));
        assert!(text.contains("Load config"));
        assert!(text.contains("Enter to cancel"));
        assert_eq!(text.matches("API base URL").count(), 2);
        assert_eq!(text.matches("Endpoint must use HTTP or HTTPS").count(), 1);
        assert!(!text.contains("Model name"));
    }

    #[test]
    fn multi_file_options_use_validated_endpoint_and_key_name() {
        let files = vec![PathBuf::from("one.opus"), PathBuf::from("two.opus")];
        let mut answers = concat!(
            "https://user:bad-password@example.org/v1\nhttps://example.org/?token=bad-query\n",
            "https://example.org/v1\n\nasr\nbad-token\nAUDIOCAPTURE_GUI_PID\nAC_KEY\nresults\n",
            "yes\nY\ny\nno\ninvalid\nyes\n"
        )
        .as_bytes();
        let mut log = Vec::new();
        let options = configure(files.clone(), &mut answers, &mut log)
            .unwrap()
            .unwrap();
        assert_eq!(options.files, files);
        assert_eq!(options.model, "asr");
        assert_eq!(options.key_env.as_deref(), Some("AC_KEY"));
        assert_eq!(options.base_url, "https://example.org/v1");
        assert_eq!(options.output_dir, PathBuf::from("results"));
        assert!(options.same_session && options.timestamps && options.chunk_long);
        assert!(options.review.is_none());
        let text = String::from_utf8(log).unwrap();
        assert!(text.contains("Timelines: shared\nTimestamps: yes\nSplit oversized audio files: yes\nAPI key configured: yes\n"));
        for secret in [
            "bad-password",
            "bad-query",
            "bad-token",
            "AUDIOCAPTURE_GUI_PID",
            "AC_KEY",
        ] {
            assert!(!text.contains(secret));
        }
    }

    #[test]
    fn batch_config_uses_the_executable_directory_and_prefers_the_gui_snapshot() {
        let root = std::env::temp_dir().join(new_id("batch_config_test").unwrap());
        fs::create_dir(&root).unwrap();
        let exe = root.join("transcribe.exe");
        let config_path = root.join(audio_capture::transcription_config::FILENAME);
        let profile = TranscriptionConfig {
            endpoint: "http://localhost:9091/api".into(),
            model: "test-recognizer".into(),
            transcript_output_folder: PathBuf::from("results"),
            request_segment_timestamps: true,
            ..Default::default()
        };
        profile.save(&config_path).unwrap();
        let config = load_config(&exe, None).unwrap().unwrap();
        assert_eq!(config.transcript_output_folder, root.join("results"));
        assert_eq!(config.model, "test-recognizer");
        assert!(config.request_segment_timestamps);

        let current = TranscriptionConfig {
            model: "current-model".into(),
            api_key_env: "TEST_API_KEY".into(),
            transcript_output_folder: PathBuf::new(),
            request_segment_timestamps: false,
            ..profile
        };
        let snapshot = current.to_gui_snapshot().unwrap();
        for reserved in [GUI_CONFIG_ENV, GUI_PID_ENV] {
            let invalid = TranscriptionConfig {
                api_key_env: reserved.to_ascii_lowercase(),
                ..current.clone()
            };
            assert!(invalid.to_gui_snapshot().is_err());
        }
        assert_eq!(
            load_config(&exe, Some(OsStr::new(&snapshot))).unwrap(),
            Some(current)
        );
        assert_eq!(load_config(&exe, Some(OsStr::new(""))).unwrap(), None);
        let bad = r#"{"api_key":"do-not-print"}"#;
        let snapshot_error = load_config(&exe, Some(OsStr::new(bad))).unwrap_err();
        assert!(!snapshot_error.contains("do-not-print"));
        fs::write(&config_path, bad).unwrap();
        assert!(load_config(&exe, None).is_err());
        fs::remove_file(&config_path).unwrap();
        assert_eq!(load_config(&exe, None).unwrap(), None);
        fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn configured_video_batch_only_prompts_for_review_and_approval() {
        let files = vec![PathBuf::from("sample.MP4")];
        let config = TranscriptionConfig {
            endpoint: "http://localhost:9091/api".into(),
            model: "test-recognizer".into(),
            ..Default::default()
        };
        for (answer, approved) in [("n\ny\n", true), ("n\nn\n", false)] {
            let mut log = Vec::new();
            let options = configure_checked(
                files.clone(),
                &mut answer.as_bytes(),
                &mut log,
                false,
                false,
                Some(config.clone()),
                |_, _| Ok(()),
            )
            .unwrap();
            assert_eq!(options.is_some(), approved);
            assert!(options.is_none_or(|options| !options.chunk_long && options.files == files));
            let preview = String::from_utf8(log).unwrap();
            assert!(preview.contains("sample.MP4 (extract audio locally)"));
            assert!(preview.contains("first audio stream"));
            assert!(!preview.contains("FFmpeg"));
        }
    }

    #[test]
    fn configured_batch_previews_effective_values_and_requires_explicit_consent() {
        let files = vec![
            PathBuf::from(r"C:\Audio\Discord.opus"),
            PathBuf::from(r"C:\Audio\Mic.opus"),
        ];
        let mut config = TranscriptionConfig {
            endpoint: "http://localhost:9091/api".into(),
            model: "test-recognizer".into(),
            api_key_env: "TEST_BATCH_KEY".into(),
            transcript_output_folder: PathBuf::from(r"D:\Text"),
            request_segment_timestamps: true,
            ..Default::default()
        };
        for approved in [false, true] {
            let input = if approved {
                "y\nn\nn\ny\n"
            } else {
                "y\nn\nn\n\n"
            };
            let mut log = Vec::new();
            let options = configure_checked(
                files.clone(),
                &mut input.as_bytes(),
                &mut log,
                false,
                false,
                Some(config.clone()),
                |_, _| Ok(()),
            )
            .unwrap();
            let preview = String::from_utf8(log).unwrap();
            assert!(preview.contains("Using transcription config."));
            assert!(preview.contains("Send to: http://localhost:9091/api/audio/transcriptions"));
            assert!(preview.contains("Model: test-recognizer"));
            assert!(preview.contains("Timestamps: yes"));
            assert!(preview.contains("API key configured: yes"));
            for absent in [
                "TEST_BATCH_KEY",
                "API base URL (",
                "Model name:",
                "Output directory [",
                "Request timestamps?",
            ] {
                assert!(!preview.contains(absent));
            }
            if let Some(options) = options {
                assert!(approved);
                assert_eq!(options.files, files);
                assert_eq!(options.base_url, config.endpoint);
                assert_eq!(options.model, config.model);
                assert_eq!(
                    options.key_env.as_deref(),
                    Some(config.api_key_env.as_str())
                );
                assert_eq!(options.output_dir, config.transcript_output_folder);
                assert!(options.same_session && options.timestamps && !options.chunk_long);
                assert!(options.review.is_none());
            } else {
                assert!(!approved);
            }
        }
        config.transcript_output_folder.clear();
        let options = configure_checked(
            files,
            &mut &b"y\nn\nn\ny\n"[..],
            &mut Vec::new(),
            false,
            false,
            Some(config),
            |_, _| Ok(()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(options.output_dir, PathBuf::from(r"C:\Audio\transcripts"));
    }

    #[test]
    fn rejects_overlong_lines_before_reading_the_rest() {
        let bytes = vec![b'x'; 1_000_000];
        let mut input = bytes.as_slice();
        assert!(read_answer(&mut input).is_err());
        assert!(input.len() > 800_000);
        assert!(read_answer(&mut &b"yes\x00\n"[..]).is_err());
    }

    #[test]
    fn session_guidance_rejects_duplicate_coverage_and_requires_approval() {
        let directory = std::env::temp_dir().join(new_id("prepare_session_test").unwrap());
        fs::create_dir(&directory).unwrap();
        let mix = directory.join("mix.opus");
        let program = directory.join("program.opus");
        let mix_text = directory.join("mix.txt");
        let program_text = directory.join("program.txt");
        for path in [&mix, &program, &mix_text, &program_text] {
            fs::write(path, []).unwrap();
        }
        let fixture = [
            mix_text.to_str().unwrap(),
            "mix",
            program_text.to_str().unwrap(),
            "program",
            "y",
            "y",
            "y",
            "n",
            "y",
        ]
        .join("\n");
        let mut output = Vec::new();
        let selected = vec![mix.clone(), program.clone()];
        let approved = format!("{fixture}\ny\n");
        let result = configure_session(selected.clone(), &mut approved.as_bytes(), &mut output)
            .unwrap()
            .unwrap();
        assert!(result.shared_clock);
        assert_eq!(result.representation, "explicit");
        assert_eq!(result.sources[0].selected, Some(false));
        assert_eq!(result.sources[1].selected, Some(true));
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("Choose a mix by itself")
        );

        let rejected = format!("{fixture}\nn\n");
        let result =
            configure_session(selected, &mut rejected.as_bytes(), &mut Vec::new()).unwrap();
        assert!(result.is_none());
        for path in [mix, program, mix_text, program_text] {
            fs::remove_file(path).unwrap();
        }
        fs::remove_dir(directory).unwrap();
    }
}
