use std::io::Cursor;

use super::*;

fn run_mcp(input: &str) -> Vec<Value> {
    let mut output = Vec::new();
    let mut dispatch = |group: &str, action: &str, args: &Value| {
        assert!(!args.as_object().unwrap().contains_key("action"));
        assert!(!args.as_object().unwrap().contains_key("grant_ref"));
        match (group, action) {
            ("diagnostics", "capabilities") => Ok(json!({"schema_version":1,"ok":true})),
            ("batch", "status") => Err((3, "Access denied".into())),
            _ => panic!("unexpected call {group}.{action}"),
        }
    };
    mcp::serve(Cursor::new(input), &mut output, &mut dispatch).unwrap();
    std::str::from_utf8(&output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

const INITIALIZE: &str = concat!(
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
    "\n",
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    "\n"
);

#[test]
fn mcp_negotiates_lists_and_dispatches_structured_result() {
    let input = format!(
        "{INITIALIZE}{}\n{}\n",
        r#"{"jsonrpc":"2.0","id":"listing","method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"audio_capture_diagnostics","arguments":{"schema_version":1,"action":"capabilities"}}}"#,
    );
    let output = run_mcp(&input);
    assert_eq!(output.len(), 3);
    assert_eq!(output[0]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(output[1]["id"], "listing");
    let tools = output[1]["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().all(|tool| {
        tool["inputSchema"]["type"] == "object"
            && tool["outputSchema"]["type"] == "object"
            && tool["name"] != "audio_capture_grants"
    }));
    assert_eq!(
        output[2]["result"]["structuredContent"],
        json!({"schema_version":1,"ok":true})
    );
    assert_eq!(output[2]["result"]["isError"], false);
}

#[test]
fn mcp_rejects_uninitialized_and_bad_calls() {
    let input = format!(
        "{}\n{INITIALIZE}{}\n{}\n{}\n{}\n",
        r#"{"jsonrpc":"2.0","id":0,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"audio_capture_batch","arguments":{"schema_version":2,"action":"status","batch_id":"id"}}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"audio_capture_grants","arguments":{"schema_version":1,"action":"provision"}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"audio_capture_batch","arguments":{"schema_version":1,"action":"status","batch_id":"id","grant_ref":"override"}}}"#,
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"audio_capture_batch","arguments":{"schema_version":1,"action":"status","batch_id":"id"}}}"#,
    );
    let output = run_mcp(&input);
    assert_eq!(output.len(), 6);
    assert_eq!(output[0]["error"]["code"], -32000);
    assert_eq!(
        output[2]["result"]["structuredContent"]["error"]["code"],
        "invalid_input"
    );
    assert_eq!(output[3]["error"]["code"], -32602);
    assert_eq!(
        output[4]["result"]["structuredContent"]["error"]["code"],
        "invalid_input"
    );
    assert_eq!(output[5]["result"]["isError"], true);
    assert_eq!(
        output[5]["result"]["structuredContent"]["error"]["code"],
        "authorization_denied"
    );
}

#[test]
fn mcp_bounds_frames_and_returns_parse_errors() {
    let input = format!("[\n{}\n", " ".repeat(MAX_REQUEST_BYTES + 1));
    let output = run_mcp(&input);
    assert_eq!(output.len(), 2);
    assert_eq!(output[0]["error"]["code"], -32700);
    assert_eq!(output[1]["error"]["code"], -32001);
}

#[test]
fn cli_reads_one_object_and_returns_usage_errors_as_json() {
    let mut output = Vec::new();
    let mut input = Cursor::new(b"\xef\xbb\xbf{\"schema_version\":1}");
    let mut dispatch = |group: &str, action: &str, req: &Value| {
        assert_eq!((group, action), ("diagnostics", "capabilities"));
        assert_eq!(req, &json!({"schema_version":1}));
        Ok(json!({"ok":true,"schema_version":1}))
    };
    assert_eq!(
        run_cli(
            ["diagnostics", "capabilities", "--json", "--input", "-"].map(OsString::from),
            &mut input,
            &mut output,
            &mut dispatch
        )
        .unwrap(),
        0
    );
    let result: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(result["ok"], true);
    output.clear();
    let mut input = Cursor::new(b"{\"schema_version\":1}{}");
    assert_eq!(
        run_cli(
            ["diagnostics", "capabilities", "--json", "--input", "-"].map(OsString::from),
            &mut input,
            &mut output,
            &mut dispatch
        )
        .unwrap(),
        2
    );
    let result: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(result["error"]["code"], "invalid_input");
}
