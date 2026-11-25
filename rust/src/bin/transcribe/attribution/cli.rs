use super::{
    adapter::{AdapterConfig, DEFAULT_TIMEOUT_SECONDS, MAX_TIMEOUT_SECONDS},
    attribute, decisions, export, importer, review, store,
};
use crate::guided;
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const PREPARE: &str =
    "Usage: transcribe prepare-session --session MANIFEST [--guided] [options] [-- AUDIO...]

Choose completed recordings and their local recognition artifacts.

  --adapter EXE           Use this local executable path
  --adapter-arg ARG       Add an adapter argument in order. Repeat as needed
  --adapter-timeout N     Adapter timeout in seconds
  --automatic-names      Display supported inferred names for this session
  --naming-context JSON  Read explicitly selected transcript evidence
";
const ATTRIBUTE: &str = "Usage: transcribe attribute --session MANIFEST [--session MANIFEST ...]

Process each selected session locally. Continue when a session fails.
A nonzero exit code indicates failure or partial results.
";
const REVIEW: &str =
    "Usage: transcribe review-session --session MANIFEST [--apply DECISION.json] [--json]

Display the current session. Apply a decision only when explicitly requested.
With --json, print the full resulting session as JSON.
";
const EXPORT: &str = "Usage: transcribe export-memory --scope SCOPE.json --output DIR [options]

Preview the selected content and pending changes by default.

  --approve     Publish the approved content to the local stream
  --preview     Show the preview without publishing
  --after N     Use a cursor from this stream
  --snapshot    Publish or preview a recovery snapshot of the approved scope
";
const IMPORT: &str = "Usage: transcribe import-memory --batch BATCH --output DIR

Apply a local memory batch to the reference importer. Repeat safely.
";
const REFINE_LIVE: &str =
    "Usage: transcribe refine-live --index FINAL.segments.jsonl --session MANIFEST
       [--timeline FINAL.timeline.jsonl --audio STEM --adapter-config CONFIG.json]

Import a finalized live interval for local review and explicit memory export.
Select a retained stem and its finalized timeline to refine speaker labels locally.
The adapter reads the selected file. Only verified mapped transcript spans can change.
Repeat for another retained stem. Existing reviewed assignments and original words remain available.
";

#[derive(Clone, Copy)]
pub enum Command {
    RefineLive,
    Prepare,
    Attribute,
    Review,
    Export,
    Import,
}

impl Command {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "refine-live" => Some(Self::RefineLive),
            "prepare-session" => Some(Self::Prepare),
            "attribute" => Some(Self::Attribute),
            "review-session" => Some(Self::Review),
            "export-memory" => Some(Self::Export),
            "import-memory" => Some(Self::Import),
            _ => None,
        }
    }

    fn syntax(self) -> (&'static str, &'static [OptionKind]) {
        use OptionKind::*;
        match self {
            Self::RefineLive => (
                REFINE_LIVE,
                &[
                    Value("--index"),
                    Value("--session"),
                    Value("--timeline"),
                    Value("--audio"),
                    Value("--adapter-config"),
                ],
            ),
            Self::Prepare => (
                PREPARE,
                &[
                    Value("--session"),
                    Value("--adapter"),
                    Repeat("--adapter-arg"),
                    Value("--adapter-timeout"),
                    Value("--naming-context"),
                    Switch("--automatic-names"),
                    Switch("--guided"),
                ],
            ),
            Self::Attribute => (ATTRIBUTE, &[Repeat("--session")]),
            Self::Review => (
                REVIEW,
                &[Value("--session"), Value("--apply"), Switch("--json")],
            ),
            Self::Export => (
                EXPORT,
                &[
                    Value("--scope"),
                    Value("--output"),
                    Value("--after"),
                    Switch("--preview"),
                    Switch("--approve"),
                    Switch("--snapshot"),
                ],
            ),
            Self::Import => (IMPORT, &[Value("--batch"), Value("--output")]),
        }
    }
}

#[derive(Clone, Copy)]
enum OptionKind {
    Switch(&'static str),
    Value(&'static str),
    Repeat(&'static str),
}

impl OptionKind {
    fn name(self) -> &'static str {
        match self {
            Self::Switch(name) | Self::Value(name) | Self::Repeat(name) => name,
        }
    }
}

#[derive(Default)]
struct Args {
    options: BTreeMap<&'static str, Vec<OsString>>,
    files: Vec<PathBuf>,
}

impl Args {
    fn parse(command: Command, mut args: impl Iterator<Item = OsString>) -> Result<Self, String> {
        let mut parsed = Self::default();
        let (_, options) = command.syntax();
        while let Some(arg) = args.next() {
            if arg == "--" && matches!(command, Command::Prepare) {
                parsed.files.extend(args.map(PathBuf::from));
                break;
            }
            let option = options
                .iter()
                .copied()
                .find(|option| arg == option.name())
                .ok_or_else(|| format!("Unknown option {}", arg.to_string_lossy()))?;
            let name = option.name();
            if parsed.options.contains_key(name) && !matches!(option, OptionKind::Repeat(_)) {
                return Err(format!("{name} can be specified only once."));
            }
            let values = parsed.options.entry(name).or_default();
            if !matches!(option, OptionKind::Switch(_)) {
                let value = args
                    .next()
                    .ok_or_else(|| format!("Missing value for {name}."))?;
                if value.is_empty() && name != "--adapter-arg" {
                    return Err(format!("Missing value for {name}."));
                }
                values.push(value);
            }
        }
        Ok(parsed)
    }

    fn switch(&self, name: &'static str) -> bool {
        self.options.contains_key(name)
    }

    fn one(&mut self, name: &'static str) -> Option<OsString> {
        self.options
            .remove(name)
            .and_then(|mut values| values.pop())
    }

    fn path(&mut self, name: &'static str) -> Result<PathBuf, String> {
        self.one(name)
            .map(PathBuf::from)
            .ok_or_else(|| format!("{name} is required."))
    }

    fn number(&mut self, name: &'static str) -> Result<Option<u64>, String> {
        self.one(name)
            .map(|value| {
                value
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| format!("{name} must be a nonnegative integer."))
            })
            .transpose()
    }

    fn many(&mut self, name: &'static str) -> Vec<OsString> {
        self.options.remove(name).unwrap_or_default()
    }
}

fn load_json(path: &Path) -> Result<Value, String> {
    // Review operations and naming context are explicitly selected control files.
    const LIMIT: u64 = 16 * 1024 * 1024;
    let mut file = File::open(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err(format!("Expected a regular file: {}", path.display()));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    if bytes.len() as u64 > LIMIT {
        return Err(format!("JSON file exceeds 16 MiB: {}", path.display()));
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid JSON in {}: {e}", path.display()))
}

fn prepare(mut args: Args) -> Result<(), String> {
    let manifest = args.path("--session")?;
    let program = args.one("--adapter");
    let arguments = args.many("--adapter-arg");
    let timeout = args.number("--adapter-timeout")?;
    if program.is_none() && (!arguments.is_empty() || timeout.is_some()) {
        return Err("--adapter-arg and --adapter-timeout require --adapter.".into());
    }
    let adapter = program
        .map(|program| {
            let program = std::path::absolute(program).map_err(|e| e.to_string())?;
            if !program.is_file() {
                return Err(format!(
                    "Cannot find local adapter executable {}.",
                    program.display()
                ));
            }
            let args = arguments
                .into_iter()
                .map(|arg| {
                    arg.into_string()
                        .map_err(|_| "Use Unicode adapter arguments.")
                })
                .collect::<Result<Vec<_>, _>>()?;
            let timeout_seconds = timeout.unwrap_or(DEFAULT_TIMEOUT_SECONDS);
            if !(1..=MAX_TIMEOUT_SECONDS).contains(&timeout_seconds) {
                return Err(format!(
                    "--adapter-timeout must be from 1 to {MAX_TIMEOUT_SECONDS} seconds."
                ));
            }
            Ok(AdapterConfig {
                program,
                args,
                timeout_seconds,
            })
        })
        .transpose()?;
    let context = args
        .one("--naming-context")
        .map(|path| load_json(Path::new(&path)))
        .transpose()?
        .unwrap_or_default();
    let automatic_names = args.switch("--automatic-names");
    guided::prepare_session(&manifest, args.files, adapter, automatic_names, context)
}

fn analyze(mut args: Args) -> Result<(), String> {
    let paths = args.many("--session");
    if paths.is_empty() {
        return Err("Supply at least one --session MANIFEST.".into());
    }
    let total = paths.len();
    let mut incomplete = 0;
    for path in paths.iter().map(Path::new) {
        match attribute(path) {
            Ok(session) => {
                let adapter_failures = session
                    .sources
                    .iter()
                    .filter(|source| source.provenance.get("adapter_error").is_some())
                    .count();
                let complete = session.complete && adapter_failures == 0;
                println!(
                    "{} revision {}. {} sources, {} speakers, {} turns. {}.",
                    session.session_id,
                    session.revision,
                    session.sources.len(),
                    session.speakers.len(),
                    session.turns.len(),
                    if complete { "Complete" } else { "Partial" },
                );
                if !complete {
                    for source in &session.sources {
                        eprintln!("  {} {}: {}", source.source_id, source.role, source.status);
                        if let Some(message) = source
                            .provenance
                            .get("adapter_error")
                            .and_then(Value::as_str)
                        {
                            eprintln!("    Adapter: {message}");
                        }
                    }
                }
                incomplete += usize::from(!complete);
            }
            Err(error) => {
                eprintln!("Cannot process {}: {error}", path.display());
                incomplete += 1;
            }
        }
    }
    if incomplete > 0 {
        return Err(format!(
            "{incomplete} of {total} sessions had incomplete results."
        ));
    }
    Ok(())
}

fn inspect(mut args: Args) -> Result<(), String> {
    let path = args.path("--session")?;
    let session = if let Some(command) = args.one("--apply") {
        let command = load_json(Path::new(&command))?;
        let revision = command
            .get("expected_revision")
            .and_then(Value::as_u64)
            .ok_or("The review decision needs an expected_revision.")?;
        review(&path, revision, command)?
    } else {
        store::latest(&path)?
    };
    if args.switch("--json") {
        let mut stdout = io::stdout().lock();
        serde_json::to_writer_pretty(&mut stdout, &session).map_err(|e| e.to_string())?;
        writeln!(stdout).map_err(|e| e.to_string())
    } else {
        print!("{}", decisions::render(&session));
        Ok(())
    }
}

fn memory_export(mut args: Args) -> Result<(), String> {
    let scope = args.path("--scope")?;
    let output = args.path("--output")?;
    let approve = args.switch("--approve");
    if approve && args.switch("--preview") {
        return Err("Choose either --preview or --approve.".into());
    }
    let after = args.number("--after")?;
    let message = if args.switch("--snapshot") {
        if after.is_some() {
            return Err("--snapshot cannot use --after.".into());
        }
        export::snapshot(&scope, &output, approve)?
    } else {
        export::run(&scope, &output, after, approve)?
    };
    println!("{message}");
    Ok(())
}

pub fn run(command: Command, args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let mut args = args.peekable();
    let (help, _) = command.syntax();
    if args
        .peek()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s == "--help" || s == "-h")
    {
        print!("{help}");
        return Ok(());
    }
    let mut args = Args::parse(command, args)?;
    match command {
        Command::RefineLive => {
            let index = args.path("--index")?;
            let session = args.path("--session")?;
            let refinement = match (
                args.one("--timeline"),
                args.one("--audio"),
                args.one("--adapter-config"),
            ) {
                (None, None, None) => None,
                (Some(map), Some(audio), Some(config)) => Some((
                    PathBuf::from(map),
                    PathBuf::from(audio),
                    PathBuf::from(config),
                )),
                _ => return Err("Select --timeline, --audio and --adapter-config together.".into()),
            };
            let result = super::live_import::run(&index, &session, refinement.as_ref())?;
            println!(
                "Live session {} revision {}. {} sources, {} turns.",
                result.session_id,
                result.revision,
                result.sources.len(),
                result.turns.len()
            );
            Ok(())
        }
        Command::Prepare => prepare(args),
        Command::Attribute => analyze(args),
        Command::Review => inspect(args),
        Command::Export => memory_export(args),
        Command::Import => {
            let batch = args.path("--batch")?;
            let output = args.path("--output")?;
            println!("{}", importer::run(&batch, &output)?);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> impl Iterator<Item = OsString> {
        args.iter().map(OsString::from)
    }

    #[test]
    fn preserves_batch_path_order_and_guided_literal_files() {
        let mut batch = Args::parse(
            Command::Attribute,
            strings(&["--session", "first.json", "--session", "second 日本語.json"]),
        )
        .unwrap();
        assert_eq!(
            batch.many("--session"),
            ["first.json", "second 日本語.json"]
        );
        let guided = Args::parse(
            Command::Prepare,
            strings(&[
                "--session",
                "draft.json",
                "--guided",
                "--",
                "--one.opus",
                "two words.wav",
            ]),
        )
        .unwrap();
        assert_eq!(
            guided.files,
            [PathBuf::from("--one.opus"), PathBuf::from("two words.wav")]
        );
    }

    #[test]
    fn rejects_ambiguous_and_cross_command_flags() {
        for flags in [
            vec!["--scope"],
            vec!["--scope", "one", "--scope", "two"],
            vec!["--session", "one"],
        ] {
            assert!(Args::parse(Command::Export, strings(&flags)).is_err());
        }
        let export = [
            "--scope",
            "one",
            "--output",
            "two",
            "--preview",
            "--approve",
        ];
        assert!(
            run(Command::Export, strings(&export))
                .unwrap_err()
                .contains("either")
        );
    }
}
