mod attribution;
mod chunks;
mod guided;
mod jobs;
mod review;
mod transport;

use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::PathBuf;

const HELP: &str =
    "Usage: transcribe --base-url URL --model MODEL --output-dir DIR [options] FILE...
       transcribe --guided -- FILE...

Transcribe completed files explicitly, one at a time.

Speaker and memory commands (use COMMAND --help for details):
  prepare-session      Create a local session manifest with guided prompts
  attribute            Analyze selected finished recordings and transcripts
  review-session       Inspect a session or apply a review decision
  export-memory        Preview or approve a local memory export
  import-memory        Apply an exported batch to a local reference importer

Options:
  --api-key-env NAME   Read the bearer token from this environment variable
  --language CODE      Request a language supported by the endpoint
  --max-file-mib N     Maximum input size in MiB (default 25)
  --allow-http         Permit HTTP for a remote endpoint
  --timestamps         Request verbose_json from the endpoint
  --provider-profile openai-diarized-v1
                       Request diarized_json from a capable endpoint and model
                       Use this option without --timestamps
  --retry-unknown      Explicitly retry requests with unknown completion
  --review PATH        Create an HTML review of the selected transcripts
  --same-session       Treat selected sources as one recording timeline
  --allow-duplicate-sources  Include a combined track and its separate tracks
  --chunk-long         Convert oversized files to bounded WAV chunks using ffmpeg
  --help               Show this help
";

fn main() {
    let mut args = std::env::args_os().skip(1).peekable();
    let guided = args.next_if(|arg| arg == "--guided").is_some();
    let result = if guided {
        guided::run(args)
    } else if let Some(command) = args
        .peek()
        .and_then(|arg| arg.to_str())
        .and_then(attribution::cli::Command::from_name)
    {
        args.next();
        attribution::cli::run(command, args)
    } else {
        run(args)
    };
    if let Err(error) = &result {
        eprintln!("{error}");
    }
    if guided && io::stdin().is_terminal() && io::stdout().is_terminal() {
        println!("Press Enter to close.");
        let _ = guided::read_answer(&mut io::stdin().lock());
    }
    if result.is_err() {
        std::process::exit(1);
    }
}

fn run(mut args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let mut options = jobs::Options::default();
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            print!("{HELP}");
            return Ok(());
        }
        if arg == "--allow-http" {
            options.allow_http = true;
        } else if arg == "--retry-unknown" {
            options.retry_unknown = true;
        } else if arg == "--timestamps" {
            options.timestamps = true;
        } else if arg == "--same-session" {
            options.same_session = true;
        } else if arg == "--allow-duplicate-sources" {
            options.allow_duplicate_sources = true;
        } else if arg == "--chunk-long" {
            options.chunk_long = true;
        } else if arg == "--" {
            options.files.extend(args.map(PathBuf::from));
            break;
        } else if arg.to_string_lossy().starts_with("--") {
            let value = args
                .next()
                .ok_or_else(|| format!("Missing value for {}", arg.to_string_lossy()))?;
            match arg.to_str() {
                Some("--base-url") => options.base_url = value.to_string_lossy().into_owned(),
                Some("--model") => options.model = value.to_string_lossy().into_owned(),
                Some("--output-dir") => options.output_dir = PathBuf::from(value),
                Some("--review") => options.review = Some(PathBuf::from(value)),
                Some("--provider-profile") => {
                    options.provider_profile = Some(
                        value
                            .to_str()
                            .ok_or("The provider profile must be valid Unicode")?
                            .parse::<transport::ProviderProfile>()?,
                    );
                }
                Some("--api-key-env") => {
                    options.key_env = Some(value.to_string_lossy().into_owned())
                }
                Some("--language") => options.language = Some(value.to_string_lossy().into_owned()),
                Some("--max-file-mib") => {
                    options.max_file_mib = value
                        .to_str()
                        .and_then(|v| v.parse::<u64>().ok())
                        .filter(|v| (1..=1024).contains(v))
                        .ok_or("--max-file-mib must be an integer from 1 to 1024")?;
                }
                _ => return Err(format!("Unknown option {}", arg.to_string_lossy())),
            }
        } else {
            options.files.push(PathBuf::from(arg));
        }
    }
    if options.base_url.is_empty()
        || options.model.is_empty()
        || options.output_dir.as_os_str().is_empty()
        || options.files.is_empty()
    {
        return Err(format!(
            "A base URL, model, output directory and at least one file are required.\n{HELP}"
        ));
    }
    jobs::run(options)
}
