//! MCP stdio transport and tool discovery for the local workflow.

use super::{
    CallResult, MAX_REQUEST_BYTES, SCHEMA_VERSION, envelope, invalid, invoke, write_message,
};
use serde_json::{Map, Value, json};
use std::io::{self, BufRead, Read, Write};

const PROTOCOL_VERSION: &str = "2025-11-25";
const SERVER_NAME: &str = "audio-capture";

struct Action {
    name: &'static str,
    required: &'static [&'static str],
}

struct Tool {
    group: &'static str,
    name: &'static str,
    description: &'static str,
    fields: &'static [&'static str],
    actions: &'static [Action],
}

const TOOLS: &[Tool] = &[
    Tool {
        group: "diagnostics",
        name: "audio_capture_diagnostics",
        description: "List available local capabilities and limits.",
        fields: &[],
        actions: &[Action {
            name: "capabilities",
            required: &[],
        }],
    },
    Tool {
        group: "profile",
        name: "audio_capture_profile",
        description: "List local speech models or import a model pack with an approved plan.",
        fields: &[
            "operation",
            "source_dir",
            "plan_id",
            "approved",
            "idempotency_key",
        ],
        actions: &[
            Action {
                name: "model_list",
                required: &[],
            },
            Action {
                name: "preview",
                required: &["operation", "source_dir"],
            },
            Action {
                name: "commit",
                required: &["plan_id", "approved", "idempotency_key"],
            },
        ],
    },
    Tool {
        group: "batch",
        name: "audio_capture_batch",
        description: "Preview, start, or inspect local MP4 transcription. Commit requires explicit approval.",
        fields: &[
            "operation",
            "profile_id",
            "files",
            "output_dir",
            "plan_id",
            "approved",
            "idempotency_key",
            "batch_id",
            "offset",
            "limit",
        ],
        actions: &[
            Action {
                name: "preview",
                required: &["operation", "profile_id", "files"],
            },
            Action {
                name: "commit",
                required: &["plan_id", "approved", "idempotency_key"],
            },
            Action {
                name: "list",
                required: &[],
            },
            Action {
                name: "status",
                required: &["batch_id"],
            },
            Action {
                name: "cancel",
                required: &["batch_id"],
            },
        ],
    },
    Tool {
        group: "transcript",
        name: "audio_capture_transcript",
        description: "Fetch a bounded page of timed text for an authorized local job.",
        fields: &["batch_id", "offset", "limit"],
        actions: &[Action {
            name: "fetch",
            required: &["batch_id"],
        }],
    },
];

pub(super) fn supports(group: &str, action: &str) -> bool {
    TOOLS
        .iter()
        .any(|tool| tool.group == group && tool.actions.iter().any(|item| item.name == action))
}

pub(super) fn serve(
    mut input: impl BufRead,
    mut output: impl Write,
    dispatch: &mut impl FnMut(&str, &str, &Value) -> CallResult,
) -> io::Result<()> {
    let mut state = Session::New;
    let mut line = Vec::new();
    loop {
        line.clear();
        let size = input
            .by_ref()
            .take((MAX_REQUEST_BYTES + 2) as u64)
            .read_until(b'\n', &mut line)?;
        if size == 0 {
            return Ok(());
        }
        if line.strip_suffix(b"\n").unwrap_or(&line).len() > MAX_REQUEST_BYTES {
            write_message(
                &mut output,
                &error(Value::Null, -32001, "The MCP message exceeds 64 KiB."),
            )?;
            // The frame may still be arriving. Close without buffering or waiting for its tail.
            return Ok(());
        }
        let reply = match serde_json::from_slice(&line) {
            Ok(message) => state.handle(message, dispatch),
            Err(_) => Some(error(Value::Null, -32700, "Parse error.")),
        };
        if let Some(reply) = reply {
            write_message(&mut output, &reply)?;
        }
    }
}

#[derive(PartialEq, Eq)]
enum Session {
    New,
    AwaitingInitialized,
    Ready,
}

impl Session {
    fn handle(
        &mut self,
        request: Value,
        dispatch: &mut impl FnMut(&str, &str, &Value) -> CallResult,
    ) -> Option<Value> {
        let id = request.get("id").cloned();
        let invalid_envelope = || Some(error(Value::Null, -32600, "Invalid request."));
        if request["jsonrpc"] != "2.0"
            || id
                .as_ref()
                .is_some_and(|id| !id.is_string() && !id.is_number())
        {
            return invalid_envelope();
        }
        let Some(method) = request["method"].as_str() else {
            return invalid_envelope();
        };
        let params = request.get("params");
        if let Some(params) = params
            && !params.is_object()
        {
            return id.map(|id| error(id, -32602, "Supply an object for params."));
        }
        let Some(id) = id else {
            if method == "notifications/initialized" && *self == Self::AwaitingInitialized {
                *self = Self::Ready;
            }
            return None;
        };
        let params = params.unwrap_or(&Value::Null);
        let result = match method {
            "ping" => Ok(json!({})),
            "initialize" => self.initialize(params),
            _ if *self != Self::Ready => Err((-32000, "Initialize the MCP connection first.")),
            "tools/list" if params.get("cursor").is_some() => {
                Err((-32602, "The tool list has one page."))
            }
            "tools/list" => Ok(json!({
                "tools": TOOLS.iter().map(Tool::definition).collect::<Vec<_>>(),
            })),
            "tools/call" => call_tool(params, dispatch),
            _ => Err((-32601, "Method not found.")),
        };
        Some(match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err((code, message)) => error(id, code, message),
        })
    }

    fn initialize(&mut self, params: &Value) -> Result<Value, (i32, &'static str)> {
        if *self != Self::New {
            return Err((-32600, "The MCP connection is already initialized."));
        }
        if params["protocolVersion"].as_str().is_none_or(str::is_empty)
            || !params["capabilities"].is_object()
            || params["clientInfo"]["name"]
                .as_str()
                .is_none_or(str::is_empty)
            || params["clientInfo"]["version"]
                .as_str()
                .is_none_or(str::is_empty)
        {
            return Err((
                -32602,
                "Supply MCP version, capabilities and client information.",
            ));
        }
        *self = Self::AwaitingInitialized;
        Ok(json!({
            "protocolVersion":PROTOCOL_VERSION,
            "capabilities":{"tools":{"listChanged":false}},
            "serverInfo":{"name":SERVER_NAME,"title":"Audio Capture","version":env!("CARGO_PKG_VERSION")}
        }))
    }
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn call_tool(
    params: &Value,
    dispatch: &mut impl FnMut(&str, &str, &Value) -> CallResult,
) -> Result<Value, (i32, &'static str)> {
    let name = params["name"]
        .as_str()
        .ok_or((-32602, "Supply a tool name."))?;
    let tool = TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .ok_or((-32602, "The requested tool is unavailable."))?;
    let mut request = params["arguments"]
        .as_object()
        .cloned()
        .ok_or((-32602, "Supply an arguments object."))?;
    let action = request.remove("action");
    let result = match action.as_ref().and_then(Value::as_str) {
        Some(name) if tool.actions.iter().any(|item| item.name == name) => {
            invoke(dispatch, tool.group, name, &Value::Object(request))
        }
        _ => Err(invalid("The tool action is unavailable.")),
    };
    let (exit, result) = envelope(result);
    Ok(json!({
        "isError":exit != 0,
        "content":[{"type":"text","text":result.to_string()}],
        "structuredContent":result
    }))
}

impl Tool {
    fn definition(&self) -> Value {
        let mut properties = Map::from_iter([
            (
                "schema_version".into(),
                json!({"type":"integer","const":SCHEMA_VERSION}),
            ),
            (
                "action".into(),
                json!({"enum":self.actions.iter().map(|action| action.name).collect::<Vec<_>>()}),
            ),
        ]);
        properties.extend(self.fields.iter().map(|field| {
            let schema = match *field {
                "files" => json!({
                    "type":"array","minItems":1,"maxItems":1,"items":{"type":"string","minLength":1}
                }),
                "approved" => json!({"type":"boolean","const":true}),
                "operation" => json!({
                    "const":if self.group == "batch" {"transcribe"} else {"import_model"}
                }),
                "profile_id" => json!({"const":"local-whisper"}),
                "offset" => json!({"type":"integer","minimum":0}),
                "limit" => json!({"type":"integer","minimum":1,"maximum":100}),
                _ => json!({"type":"string","minLength":1}),
            };
            ((*field).into(), schema)
        }));
        let variants: Vec<_> = self.actions.iter().map(|action| {
            json!({"properties":{"action":{"const":action.name}},"required":action.required})
        }).collect();
        json!({
            "name":self.name,
            "description":self.description,
            "inputSchema":{
                "type":"object",
                "required":["schema_version","action"],
                "properties":properties,
                "oneOf":variants
            },
            "outputSchema":{
                "type":"object",
                "required":["schema_version","ok"],
                "properties":{
                    "schema_version":{"type":"integer","const":SCHEMA_VERSION},
                    "ok":{"type":"boolean"},
                    "error":{
                        "type":"object",
                        "required":["code","message"],
                        "properties":{"code":{"type":"string"},"message":{"type":"string"}}
                    }
                },
                "if":{"properties":{"ok":{"const":false}}},
                "then":{"required":["error"]}
            }
        })
    }
}
