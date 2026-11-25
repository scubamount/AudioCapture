use crate::{jobs, transport};
use std::ffi::OsString;
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run(args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let files = selected_files(args)?;
    let selection = configure(files, &mut io::stdin().lock(), &mut io::stdout().lock());
    match selection {
        Ok(Some(options)) => {
            let review = options.review.clone();
            jobs::run(options)?;
            println!("Transcription complete.");
            if let Some(path) = review {
                println!("Open the local review: {}", path.display());
            }
            Ok(())
        }
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

fn configure(
    files: Vec<PathBuf>,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<Option<jobs::Options>> {
    let mut ui = Prompts { input, output };
    writeln!(
        ui.output,
        "Transcribe selected recordings. Press Ctrl+C to cancel."
    )?;
    let (base_url, destination) = loop {
        let base = ui.ask("API base URL (HTTPS or loopback HTTP, without credentials):")?;
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
        if name.is_empty() {
            break None;
        }
        if name
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
        {
            break Some(name);
        }
        writeln!(
            ui.output,
            "Use letters, digits, and underscores. Start with a letter or underscore."
        )?;
    };
    let default_dir = files[0].with_file_name("transcripts");
    let dir = ui.ask(&format!("Output directory [{}]:", default_dir.display()))?;
    let output_dir = if dir.is_empty() {
        default_dir
    } else {
        dir.into()
    };
    let same_session =
        files.len() > 1 && ui.confirm("Do these files share one recording timeline?", false)?;
    let timestamps = ui.confirm("Request timestamps?", false)?;
    let chunk_long = ui.confirm("Split files over 25 MiB with FFmpeg?", false)?;
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
    for file in &files {
        writeln!(ui.output, "  {}", file.display())?;
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
    writeln!(ui.output, "Split oversized files: {}", yes_no(chunk_long))?;
    writeln!(
        ui.output,
        "API key configured: {}",
        yes_no(key_env.is_some())
    )?;
    if let Some(path) = &review {
        writeln!(ui.output, "Local HTML review: {}", path.display())?;
    }
    if !ui.confirm("Send the selected recordings for transcription?", false)? {
        return Ok(None);
    }
    Ok(Some(jobs::Options {
        files,
        base_url,
        model,
        key_env,
        output_dir,
        same_session,
        timestamps,
        chunk_long,
        review,
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
            assert!(text.contains("Timelines: separate\nTimestamps: no\nSplit oversized files: no\nAPI key configured: no\n"));
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
    fn multi_file_options_use_validated_endpoint_and_key_name() {
        let files = vec![PathBuf::from("one.opus"), PathBuf::from("two.opus")];
        let mut answers = concat!(
            "https://user:bad-password@example.org/v1\nhttps://example.org/?token=bad-query\n",
            "https://example.org/v1\n\nasr\nbad-token\nAC_KEY\nresults\n",
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
        assert!(text.contains("Timelines: shared\nTimestamps: yes\nSplit oversized files: yes\nAPI key configured: yes\n"));
        for secret in ["bad-password", "bad-query", "bad-token", "AC_KEY"] {
            assert!(!text.contains(secret));
        }
    }

    #[test]
    fn rejects_overlong_lines_before_reading_the_rest() {
        let bytes = vec![b'x'; 1_000_000];
        let mut input = bytes.as_slice();
        assert!(read_answer(&mut input).is_err());
        assert!(input.len() > 800_000);
        assert!(read_answer(&mut &b"yes\x00\n"[..]).is_err());
    }
}
