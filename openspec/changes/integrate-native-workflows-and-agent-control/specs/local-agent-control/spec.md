## Purpose

Let authorized local clients operate the recorder and its durable jobs through versioned headless commands and MCP tools.

## ADDED Requirements

### Requirement: Shared application operations

The application SHALL expose its product operations through a typed Rust command core. The native UI, headless CLI, and MCP SHALL use that core with the same validation and state transitions.
The control contract SHALL version operation and result schemas. It SHALL use stable IDs and explicit errors for unsupported operations or contract versions.

#### Scenario: Operate without a window

- **WHEN** a caller uses `transcribe.exe control` or launches `transcribe.exe mcp` with an authorized local grant
- **THEN** the caller can manage profiles and models, sources, independent recording and transcription, batches, transcripts and recovery, speaker review, memory, and diagnostics
- **AND** the caller can finish those actions without a desktop, an interactive prompt, or a raw shell command

#### Scenario: Connect to an existing session

- **WHEN** the recorder and an authorized agent use the same resolved application state directory and Windows logon session
- **THEN** one local owner serializes capture changes and provides current status to both clients
- **AND** a caller uses the same IDs to inspect the work after it reconnects
- **AND** the app can open its UI during an active headless session without creating a duplicate capture or changing its authorization

#### Scenario: Use an existing script

- **WHEN** an operator invokes an existing documented worker command
- **THEN** its options, exit behavior, and stored artifacts remain compatible
- **AND** the new structured interface uses an explicit subcommand

### Requirement: Scoped local grants

The service SHALL use opt-in grants scoped to a Windows logon session. It SHALL enforce separate status, sensitive read, and mutation rights for approved devices, processes, operations, paths, providers, and profiles.

#### Scenario: Provision or revoke access

- **WHEN** the local operator uses native settings or the operator-only headless setup command
- **THEN** the operator can create, narrow, or revoke an agent grant
- **AND** an agent tool cannot grant itself additional access

#### Scenario: Inspect progress without exposing private content

- **WHEN** an authorized caller lists sessions, jobs, or recovery items or checks their status
- **THEN** the response returns identifiers, state, progress, and any scoped next action
- **AND** reading transcript text, speaker content, output file paths, or export material requires an applicable read grant and an explicit fetch action
- **AND** responses and diagnostics exclude secret values

### Requirement: Commit immutable plans

Starting capture or playback, transferring content, exporting memory, and discarding recovery data SHALL require an approved plan. The plan SHALL bind the operation and its effective scope, inputs, and destination.

#### Scenario: Approve an agent action without the UI

- **WHEN** an authorized caller previews an in-scope operation and commits the exact valid plan with explicit approval
- **THEN** the shared core accepts it without opening a dialog
- **AND** commits for an expired plan, a changed input or destination, or an out-of-scope source are rejected before the operation starts
- **AND** no capture, upload, export, or deletion starts during discovery, validation, or preview

#### Scenario: Retry a state change after a lost reply

- **WHEN** a caller repeats a commit with the same idempotency key and exact operation payload
- **THEN** the application returns the original operation receipt without repeating the side effect
- **AND** a different payload with the same key returns a conflict
- **AND** a remote request with an unknown outcome follows the explicit retry approval path

### Requirement: Explicit work ownership

The application SHALL give approved batches durable state independently of the client connection. Sole headless capture SHALL require a renewable lease with a configurable lifetime and published upper bound.

#### Scenario: Agent disconnects during work

- **WHEN** an agent disconnects while an approved batch is active
- **THEN** the active owner continues the approved batch and an authorized client can later inspect its progress
- **AND** the disconnect leaves GUI-owned recording and transcription active
- **AND** a headless-owned interval stops intake and starts finalization when its lease expires without renewal

#### Scenario: Owner exits or Windows restarts

- **WHEN** the owning process disappears before finalization
- **THEN** completed results and recoverable partial outputs remain available
- **AND** durable batch state reports the items that require explicit resume or a separate unknown-outcome retry
- **AND** reconnecting does not start capture or resume the batch by itself

### Requirement: Machine-readable control transport

The CLI SHALL provide versioned JSON results and errors with documented exit codes. The MCP server SHALL negotiate a supported protocol revision and expose typed tools over stdio. Local IPC SHALL reject remote clients and enforce same-user, same-logon-session peer checks. It SHALL use no TCP listener.

#### Scenario: Connect a standards-compliant MCP client

- **WHEN** a client initializes the server and calls a listed tool with valid arguments
- **THEN** the server returns a structured result or typed error over newline-delimited JSON-RPC
- **AND** stdout contains MCP messages only
- **AND** long work returns a stable job or session ID before it completes and exposes bounded, cursor-based progress

#### Scenario: Send an unauthorized or malformed call

- **WHEN** a client sends an oversized, malformed, early, or unauthorized request
- **THEN** the appropriate protocol or application error is reported without starting work
- **AND** a slow client cannot stall audio capture, hold a scheduler permit, or cause unbounded event buffering
