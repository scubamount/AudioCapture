# Local control contract

## Common core

The product command core owns validation, consent, and state transitions. Use the existing capture session, queue, recognition, recovery, attribution, and memory operations. Move product functions behind typed commands before adapting the entry points.

The native window calls the same core as `transcribe.exe control` and `transcribe.exe mcp`. Reuse the packaged worker executable for an on-demand, windowless owner. Keep capture callbacks, disk and model work, and agent responses on separate paths.

Use one owner per Windows logon session and resolved application state directory. Two portable copies that resolve to the same state directory use the same owner. Report incompatible negotiation without starting a second owner. Start the owner on demand and release it when clients, leases, and active work end.

Windows named pipe ACLs restrict the channel to the provisioned local logon. Verify peer identity, reject remote connections, and reserve the first pipe instance for the owner. Use its locks to serialize state changes. [Windows pipe access rights](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights) · [Windows pipe creation](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea)

## Operations

Group and action names are stable in version 1. CLI syntax is `transcribe.exe control GROUP ACTION --json --input -`. Read one JSON object from stdin when specified. For MCP, use the tool name below and put the action and its fields in the tool arguments. The same request and result fields apply on both surfaces.

| CLI group | MCP tool | Actions |
| --- | --- | --- |
| `profile` | `audio_capture_profile` | `list`, `get`, `validate`, `save`, `remove`, `model_list`, `preview`, `commit` |
| `sources` | `audio_capture_sources` | `list`, `get`, `recent_list`, `recent_update`, `recent_remove`, `preset_list`, `preset_get`, `preset_save`, `preset_remove` |
| `session` | `audio_capture_session` | `list`, `status`, `preview`, `commit`, `pause`, `resume`, `mark`, `stop`, `renew` |
| `batch` | `audio_capture_batch` | `list`, `status`, `preview`, `commit`, `pause`, `resume`, `cancel` |
| `transcript` | `audio_capture_transcript` | `list`, `status`, `fetch`, `recovery_list`, `preview`, `commit`, `playback_stop` |
| `speaker` | `audio_capture_speaker` | `status`, `fetch`, `preview`, `commit` |
| `memory` | `audio_capture_memory` | `status`, `fetch`, `preview`, `commit` |
| `diagnostics` | `audio_capture_diagnostics` | `capabilities`, `status`, `endpoint_check` |

Preview requests use a named `operation`. Commit uses the returned plan. Configure local speech models through `profile preview` with `import_model`, `download_model`, `activate_model`, or `remove_model`. A download resolves an exact catalog artifact and verifies its digest. Validate profiles without a network request. An explicit `diagnostics endpoint_check` contacts only the authorized configured service. It reports the checks the service supports and sends no private media.

Use `session preview` with `start` to select recording, live transcription, monitoring, or an allowed combination. Use `reconfigure` to change sources or effective settings with fresh approval where required. A preset supplies selection defaults and never starts work. Start and stop each activity independently. Pause and resume recording and insert markers within the active session. Source discovery uses stable device or process identity and checks identity again at start.

`batch preview` with `transcribe` accepts completed media or finalized session files and local or configured remote profiles. Report effective alignment, output options, and skipped items. Use `retry_unknown` for fresh retry approval. Plain `resume` can restart only eligible known work.

`transcript preview` supports `publish_recovery`, `discard_recovery`, `apply_text_correction`, `export`, and `playback`. Corrections preserve raw text and create a new review revision. Export covers selected text, timing, and supported artifacts. Playback requires a separate grant and approval for the exact artifact and destination device. Fetching text cannot start playback. `playback_stop` ends playback independently of recording and transcription.

`speaker preview` supports `prepare`, `analyze`, `refine_live`, and `apply_correction`. `memory preview` supports `export` and `import`. The commit applies the approved scope only.

Use `diagnostics capabilities` for supported contract versions, available features, and configured limits. Use status for IDs, readiness, coverage, progress, and state changes. Use explicit fetch actions for transcript content, speaker details, or artifact paths. Fetch requires a read grant for that object. Use an opaque cursor and a bounded page size. Publish a continuation cursor when more data exists. Report expiry or a stale revision so clients can refresh. Status uses IDs and counters by default. Keep sensitive fields out of logs, errors, and unsolicited events.

## Grants, approval, and work ownership

Provision a grant in native settings or through a documented headless setup command run by the local operator. Store the grant in per-user state protected by Windows. Configure the client with an opaque grant reference. Use separate read and mutation rights. The grant lists allowed input and output roots, sources, operations, profiles, and permission to approve an action without a UI.

Keep grant provisioning and revocation outside the agent tool surface. Host the agent without access to the operator's provisioning credentials or grant store. An agent with unrestricted access to the operator's account has that account's privileges.

Run this command from the trusted operator context with a policy file the operator has reviewed. Its output supplies the grant reference. The command requires operator authority from the local provisioning context.

```powershell
.\transcribe.exe control grants provision --json --policy .\capture-agent-policy.json
```

To revoke access, run `transcribe.exe control grants revoke --json --grant GRANT_REF` in the same trusted context. Startup and capability discovery never provision a grant or request broader access.

Check the actual resolved source and output roots, including Windows reparse points. Check process identity, device scope, and the exact provider and model when starting an action. Keep credential references local. File selections, model imports, and availability checks neither start capture nor try a remote provider. In agent mode, the application requests no desktop access and offers no shell tool.

Return an expiring `plan_id` and effective scope after preview. Commit requires the `plan_id`, `approved: true`, and a per-grant `idempotency_key`. Bind the plan to the grant, action, sources, input identity, profile revision, destination, and output scope. Recheck all of these at commit and check held input identity before processing. Persist the outcome needed to identify a repeated commit. Unknown provider outcomes need new retry approval.

Return an ID on acceptance and let clients query progress. A durable batch continues after its submitting client closes. If the owner crashes, preserve existing recovery and explicit resume behavior. GUI activity does not belong to the MCP connection. A headless-only start grants a lease for the requested activities. A caller can renew it within configured bounds. On expiry, stop intake and finalize or leave recoverable output. A clean explicit stop works even while other clients are connected.

## Protocol and compatibility

Use MCP revision `2025-11-25` for version 1. The client starts `transcribe.exe mcp` with redirected stdin and stdout. Implement `initialize`, `notifications/initialized`, `ping`, `tools/list`, and `tools/call`. Advertise supported capabilities only. Publish JSON Schemas for arguments and structured tool results. Return standard JSON-RPC errors for protocol failures and tool results with `isError` for application failures. Use newline-delimited UTF-8 JSON-RPC on stdout and redacted diagnostics on stderr. Closing stdin ends that connection. Treat cancellation of a tool request separately from cancelling a durable job. [MCP lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) · [MCP stdio transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) · [MCP tool contract](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)

In a client that supports an `mcpServers` config, set the absolute executable path. Replace the placeholder with the operator's grant reference. The CLI reads the same reference from its environment.

```json
{
  "mcpServers": {
    "audio-capture": {
      "command": "C:\\Apps\\AudioCapture\\transcribe.exe",
      "args": ["mcp"],
      "env": {"AUDIO_CAPTURE_GRANT_REF": "grant_from_local_operator"}
    }
  }
}
```

CLI and MCP return result objects with `schema_version`, stable IDs, and named states. CLI JSON uses an `ok` field and a structured `error` on failure. Exit codes are `0` for a successful command or accepted job, `2` for usage or invalid input, `3` for authorization failure, and `1` for other command failures. Job completion is a separate state. Preserve documented existing CLI modes and persisted formats. Ignore additive fields according to the selected result version and reject an unsupported major version.

## Example

This PowerShell 7 example previews local video transcription with an existing authorized profile. Inspect the destination and output folder before the commit.

```powershell
$request = @{
    schema_version = 1
    operation = 'transcribe'
    profile_id = 'local-whisper'
    files = @('C:\Media\Meeting.mp4')
}
$preview = $request | ConvertTo-Json -Compress |
    .\transcribe.exe control batch preview --json --input - |
    ConvertFrom-Json
if (!$preview.ok) { throw $preview.error.message }
$preview.scope
```

Commit that approved plan and query the receipt. Keep the same idempotency key when retrying this exact commit.

```powershell
$approval = @{
    schema_version = 1
    plan_id = $preview.plan_id
    approved = $true
    idempotency_key = [guid]::NewGuid().ToString()
}
$receipt = $approval | ConvertTo-Json -Compress |
    .\transcribe.exe control batch commit --json --input - |
    ConvertFrom-Json
if (!$receipt.ok) { throw $receipt.error.message }
@{ schema_version = 1; batch_id = $receipt.batch_id } | ConvertTo-Json -Compress |
    .\transcribe.exe control batch status --json --input -
```

An initialized MCP client can perform the same operation. Submit the returned plan ID in the next call after approval.

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"audio_capture_batch","arguments":{"schema_version":1,"action":"preview","operation":"transcribe","profile_id":"local-whisper","files":["C:\\Media\\Meeting.mp4"]}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"audio_capture_batch","arguments":{"schema_version":1,"action":"commit","plan_id":"plan_from_preview","approved":true,"idempotency_key":"139b170d-1c9f-4225-8a42-538f312045a8"}}}
```

## Acceptance

- Validate the MCP handshake and tool schemas with a standard client. Exercise CLI with redirected stdin and stdout. Fail the test if stdout contains prompts or log text.
- Submit the same generated MP4 through native UI, CLI, and MCP test consumers. Compare effective plans, IDs for a retried commit, state transitions, and audio-only behavior.
- Inject denied grant, changed file, changed profile, reused idempotency key, expired plan, and lost response. Verify denied work has no side effects and repeats create one accepted job.
- Reopen clients while a batch runs. Disconnect a sole headless capture client and expire its lease. Verify finalized or recoverable output and preserve a parallel GUI-owned activity.
- Enforce message and pagination limits. Disconnect a slow client under capture load and check that controls, writer, and scheduler continue.
