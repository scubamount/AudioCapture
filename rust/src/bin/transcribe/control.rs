//! Noninteractive adapters for the local job service.

mod mcp;

#[cfg(test)]
mod tests;

use serde_json::{Value, json};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const SCHEMA_VERSION: u64 = 1;

type CallResult = Result<Value, (i32, String)>;

pub(super) fn cli(args: impl IntoIterator<Item = OsString>) -> i32 {
    match run_cli(
        args,
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        &mut super::local_batch::dispatch,
    ) {
        Ok(code) => code,
        Err(_) => {
            eprintln!("Cannot write the local control response.");
            1
        }
    }
}

pub(super) fn serve_mcp(args: impl IntoIterator<Item = OsString>) -> i32 {
    if args.into_iter().next().is_some() {
        eprintln!("Usage: audio-capture mcp");
        return 2;
    }
    if mcp::serve(
        io::stdin().lock(),
        io::stdout().lock(),
        &mut super::local_batch::dispatch,
    )
    .is_err()
    {
        // The client can close its output pipe first on shutdown.
        return 1;
    }
    0
}

fn run_cli(
    args: impl IntoIterator<Item = OsString>,
    input: &mut impl Read,
    output: &mut impl Write,
    dispatch: &mut impl FnMut(&str, &str, &Value) -> CallResult,
) -> io::Result<i32> {
    let result = parse_cli(args, input)
        .and_then(|(group, action, request)| invoke(dispatch, &group, &action, &request));
    let (exit, message) = envelope(result);
    write_message(output, &message)?;
    Ok(exit)
}

fn parse_cli(
    args: impl IntoIterator<Item = OsString>,
    input: &mut impl Read,
) -> Result<(String, String, Value), (i32, String)> {
    const USAGE: &str = "Use control GROUP ACTION --json [--input -].";

    let mut args = args.into_iter();
    let mut next_name = || {
        args.next()
            .and_then(|part| part.into_string().ok())
            .filter(|part| !part.starts_with('-'))
            .ok_or_else(|| invalid(USAGE))
    };
    let group = next_name()?;
    let action = next_name()?;
    enum Source {
        Default,
        Stdin,
        Policy(OsString),
        Grant(String),
    }
    let mut source = Source::Default;
    let mut json_set = false;
    while let Some(option) = args.next() {
        let vacant = matches!(source, Source::Default);
        match option.to_str() {
            Some("--json") if !json_set => json_set = true,
            Some("--input") if vacant => {
                if args.next().as_deref() != Some(OsStr::new("-")) {
                    return Err(invalid("Use --input - to read a JSON request from stdin."));
                }
                source = Source::Stdin;
            }
            Some("--policy") if vacant && group == "grants" && action == "provision" => {
                let path = args
                    .next()
                    .ok_or_else(|| invalid("Supply a policy file."))?;
                source = Source::Policy(path);
            }
            Some("--grant") if vacant && group == "grants" && action == "revoke" => {
                let grant = args
                    .next()
                    .and_then(|part| part.into_string().ok())
                    .filter(|part| !part.is_empty() && !part.starts_with('-'))
                    .ok_or_else(|| invalid("Supply a grant reference."))?;
                source = Source::Grant(grant);
            }
            _ => return Err(invalid(USAGE)),
        }
    }
    if !json_set {
        return Err(invalid(USAGE));
    }
    if !mcp::supports(&group, &action)
        && !matches!(
            (group.as_str(), action.as_str()),
            ("grants", "provision" | "revoke")
        )
    {
        return Err(invalid("This local control action is unavailable."));
    }
    let request = match source {
        Source::Default => json!({"schema_version":SCHEMA_VERSION}),
        Source::Stdin => read_json(input)?,
        Source::Policy(path) => {
            let policy = File::open(path).map_err(|_| invalid("Cannot open the policy file."))?;
            json!({"schema_version":SCHEMA_VERSION,"policy":read_json(policy)?})
        }
        Source::Grant(grant) => json!({"schema_version":SCHEMA_VERSION,"grant_ref":grant}),
    };
    Ok((group, action, request))
}

fn read_json(input: impl Read) -> Result<Value, (i32, String)> {
    let mut bytes = Vec::new();
    input
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("Cannot read the JSON request."))?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(invalid("The JSON request exceeds 64 KiB."));
    }
    let data = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    serde_json::from_slice(data).map_err(|_| invalid("Supply one valid JSON request object."))
}

fn invoke(
    dispatch: &mut impl FnMut(&str, &str, &Value) -> CallResult,
    group: &str,
    action: &str,
    request: &Value,
) -> CallResult {
    if !request.is_object() {
        return Err(invalid("Supply one JSON request object."));
    }
    if request["schema_version"].as_u64() != Some(SCHEMA_VERSION) {
        return Err(invalid("Set schema_version to 1."));
    }
    if group != "grants" && request.get("grant_ref").is_some() {
        return Err(invalid("Configure the grant in AUDIO_CAPTURE_GRANT_REF."));
    }
    dispatch(group, action, request)
}

fn invalid(message: &str) -> (i32, String) {
    (2, message.into())
}

fn envelope(result: CallResult) -> (i32, Value) {
    match result {
        Ok(result) => (0, result),
        Err((status, message)) => {
            let (exit, code) = match status {
                2 => (2, "invalid_input"),
                3 => (3, "authorization_denied"),
                _ => (1, "operation_failed"),
            };
            (
                exit,
                json!({
                    "ok":false,
                    "schema_version":SCHEMA_VERSION,
                    "error":{"code":code,"message":message}
                }),
            )
        }
    }
}

fn write_message(output: &mut impl Write, message: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, message)?;
    output.write_all(b"\n")?;
    output.flush()
}
