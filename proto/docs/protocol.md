# enzarb mux protocol, version 1

One websocket carries many multiplexed channels between a client (browser `ui`, `proxy`, workflows `executor`) and `sandbox-init`. Reference implementation: `crates/enzarb-proto`. Shared test vectors: `vectors/`.

## 1. Transport

- Each frame is sent as exactly **one websocket binary message**. Text messages are a protocol violation.
- A byte-stream decoder (`Frame::decode`) exists for tests and non-websocket transports; the frame layout is identical.
- Any frame error (section 8) is fatal for the connection.

## 2. Frame layout

All integers are big-endian. The header is 10 bytes.

```
offset size field
0      1    type      1 CTRL, 2 DATA_BIN, 3 DATA_JSON, 4 WINDOW, 5 CLOSE
1      1    flags     must be 0 (reserved; non-zero is an error)
2      4    channel   u32
6      4    length    u32, payload bytes, <= 1 MiB (1,048,576)
10     n    payload
```

| Type | Channel | Payload |
|---|---|---|
| `CTRL` (1) | must be 0 | UTF-8 JSON object tagged by `"type"` (section 5) |
| `DATA_BIN` (2) | != 0 | opaque bytes, length >= 1 (pty I/O; the `acp` NDJSON byte stream) |
| `DATA_JSON` (3) | != 0 | UTF-8 JSON value, length >= 1 (e.g. transcript lines) |
| `WINDOW` (4) | != 0 | `u32` credit in bytes, 1..=2^31-1 |
| `CLOSE` (5) | != 0 | empty (code 0), or `u16` code + optional UTF-8 reason |

The codec does not parse `DATA_*` payloads; the channel's owner does.

`CLOSE` codes: 0 normal, 1 protocol error, 2 going away, 3 cancelled. Other values are opaque.

## 3. Channels

- Channel **0** is control. Only `CTRL` frames use it.
- Channels 1..=u32::MAX are data channels. The **client** picks the id in `open` (must be unused). The server may refuse with `open_error`.
- A channel exists from `open` until both directions have sent `CLOSE`. After that the id may be reused.

## 4. Flow control (credit based)

- Each data channel has an independent **window per direction**. Both start at `INITIAL_WINDOW` = 262,144 bytes (256 KiB) as soon as the channel is open (when the server's `opened` is sent/received).
- Only `DATA_BIN` and `DATA_JSON` **payload bytes** consume credit. `CTRL`, `WINDOW`, `CLOSE` are free.
- A sender must not send a data frame larger than its remaining credit. Frames up to 1 MiB are legal framing, but a sender must split data to fit its credit (and should prefer frames <= 16 KiB).
- A receiver that gets more bytes than it granted must treat it as a violation: send `error` (`flow_control`), `CLOSE` the channel with code 1, and may drop the connection.
- `WINDOW` adds credit for the *sender of the data* (i.e. it travels opposite to the data). A receiver grants credit back **after the application consumed the bytes** (not when they arrive), so a slow consumer applies backpressure. The reference state machine batches grants once at least half the initial window has been consumed.
- Total credit held by a sender may never exceed 2^31-1; a grant that would exceed it is a violation.
- A `WINDOW` for a channel that is locally closed or unknown is ignored.

### Close semantics

- `CLOSE` is a **half-close**: the sender will send no more data on the channel. The peer may keep sending until it also sends `CLOSE`.
- A sender must not send data after its own `CLOSE`; a receiver treats data after the peer's `CLOSE` as a violation.
- Closing a pty channel from the client side also terminates the process; when the process exits, the server sends `CLOSE` (reason is `exit:<code>` for a normal exit or `signal:<n>` when killed by signal `n`, e.g. `exit:0`, `signal:9`; clients must tolerate other text).
- `acp` channels (section 12): the server closes with a reason such as `idle`, `superseded`, `shutdown` or `limit` (or `exit:<n>` / `signal:<n>` when the agent process ends). Receivers must tolerate any other reason text.
- Data in flight when `CLOSE` arrives is still delivered in order (frames are ordered per connection).

## 5. CTRL messages

JSON objects with a `"type"` field, channel 0. Unknown fields are ignored; an unknown `type` is answered with `error` (`unknown_message`). Optional fields may be absent or `null`; encoders omit `None`. Definitions mirror contracts section 4 and `crates/enzarb-proto/src/ctrl.rs`.

| Message | Direction | Fields |
|---|---|---|
| `hello` | client -> server | `versions: [u16]`, `client: "ui" \| "proxy" \| "executor"`, `features?: [string]` |
| `welcome` | server -> client | `version: u16`, `features?: [string]` |
| `open` | client -> server | `channel`, `kind: "pty" \| "acp"`, `cols`, `rows`, `command?: [string]`, `repo?: string`, `resume?: string`, `claude_token?: string` |
| `opened` | server -> client | `channel` |
| `open_error` | server -> client | `channel`, `message`, `code?: string` |
| `resize` | client -> server | `channel`, `cols`, `rows` |
| `step_start` | client -> server (only after `steps`) | `StepStart` (below). Idempotent on `run_id`: a repeat is answered `step_accepted{existing: true}` and replays the run. Exceptions, which continue the run with the new secrets (same workspace, same agent session via `session/load`, state `running`, events continuing the `seq`): an `interrupted` run, and a `finished` run whose `step_finished` carried `environment` (`disk_full`, `oom`). A run that finished for code reasons is only replayed |
| `step_accepted` | server -> client | `run_id`, `state: StepPhase`, `existing: bool` |
| `step_status` | client -> server | `run_id`, `after_seq?: u64` (default 0). The server replays `session_event`s with `seq > after_seq`, then answers `step_state`, and subscribes the session to the run's further `session_event`s and `step_finished` |
| `step_state` | server -> client (reply to `step_status`) | `run_id`, `state: StepPhase`, `result?: StepFinished` (terminal steps) |
| `step_steer` | client -> server | `run_id`, `text`: a user turn into the running agent session (queued after the current turn) |
| `step_stop` | client -> server | `run_id`: stop at the next checkpoint; the step finishes with exit `interrupted` |
| `step_resume` | client -> server | `run_id`: continue an `interrupted` step by re-attaching to its saved agent session (ACP `session/load`) |
| `session_event` | server -> client | `SessionEvent` (below), pushed to subscribed sessions |
| `step_finished` | server -> client | `StepFinished` (below); may arrive more than once, receivers ingest it idempotently |
| `credential_captured` | server -> client | `kind: "claude_oauth"`, `token?: string`, `legacy?: bool` |
| `credential_status` | client -> server | none; asks whether the sandbox holds a credential |
| `credential_state` | server -> client (reply to `credential_status`); client -> server (ack of a `legacy` `credential_captured`) | `kind: "claude_oauth"`, `configured: bool`, `scope?: string` (known: `"user"`; unknown values must be tolerated) |
| `stats_request` | client -> server | none; asks for a utilization sample. Send only after `welcome.features` contains `stats`. [ADR-0019](../../docs/adr/0019-sandbox-utilization-polling.md) |
| `stats` | server -> client (reply to `stats_request`) | `ts_ms` (unix ms, the sandbox's clock) and optional non-negative integers: counters `cpu_usage_usec`, `cpu_user_usec`, `cpu_system_usec` (`cpu.stat`), `io_read_bytes`, `io_write_bytes` (`io.stat`, summed over devices); gauges `memory_bytes` (`memory.current`), `memory_limit_bytes` (`memory.max`; omitted when unlimited), `memory_inactive_file_bytes` (`memory.stat` `inactive_file`), `storage_used_bytes`, `storage_total_bytes` (statfs of the workspace). Counters are cumulative and restart near 0 when the sandbox container restarts (new cgroup); absent means "not reported", never 0 |
| `credential_clear` | client -> server | `kind: "claude_oauth"`; no reply. A peer that does not know it answers `unknown_message` |

`claude_token` (on `open`) and `credential_captured.token` carry a per-user Claude token as a plain string. They are optional and additive (protocol version stays 1; old peers ignore them). Implementations must not log them: the Rust `Secret` type and the TS `Secret` class redact `Debug`/`inspect`/`String()`/`JSON.stringify`, and the wire encoder writes the plain value.
| `error` | both | `code`, `message`, optional `run_id`, optional `channel` (omitted when absent) |

### Steps (ADR-0022)

`StepPhase` is `running | interrupted | finished | unknown` (`unknown`: the sandbox does not know the run).

`StepStart`: `{run_id, attempt, task_id, workspaces: [StepWorkspace], exec: StepExec, outputs?: {[name]: StepOutput}, env?: {[NAME]: string}, github_token?: string}`.

- `StepWorkspace`: `{repo: "owner/repo", url, mode: "read" | "write", branch, base, push?: bool, token: string}`. Checked out under `/workspace/tasks/<task_id>/<owner>/<repo>`; the first workspace is the primary one (working directory). `branch` is created from `origin/<base>` when missing remotely. With `push`, the sandbox commits all changes and pushes `branch` with a lease on the remote head it saw when the step started.
- `StepExec`: `{kind: "agent", runtime: Runtime, model?, prompt, result_schema?}` or `{kind: "command", run, tools?: [mise tool spec]}`. `Runtime`: `{name, install?: [mise tool spec], protocol: "acp" | "stdio-json", command: [argv], env?, capabilities?}`. With `result_schema`, the agent is offered an MCP tool `submit_result` whose input schema it is (validated at call time).
- `StepOutput`: `{file, format?: "markdown" | "json" | "text", optional?: bool}`, relative to the primary workspace; `json` outputs are parsed.
- `env` values, workspace tokens and `github_token` are secrets: implementations never log or persist them, inject them into the step's process only (`github_token` as `GH_TOKEN`), and redact them from session events. The Rust `Secret` and TS `Secret` types redact them in logs.

`SessionEvent`: `{run_id, seq, at: RFC 3339, kind, payload: any, truncated?: bool}`. `seq` starts at 1 and is dense per run. Kinds (open set): `prompt`, `user`, `assistant`, `thinking`, `tool_call`, `tool_result`, `usage`, `steer`, `system`, `stdout`, `stderr`. Payloads are capped (head and tail kept, `truncated: true`).

`StepFinished`: `{run_id, exit: "success" | "failure" | "interrupted", outputs?: {[name]: any}, result?: any, git?: [StepGit], usage?: Usage, environment?: StepEnvironment, error?: string}`. `StepEnvironment`: `{kind, detail}`; optional and additive (omitted when absent, old decoders ignore it); the step failed because of its environment, not the code. `detail` is free text saying what was observed. Decoders must accept any `kind` string (an unrecognised one is `unknown`).

| `environment.kind` | Meaning (typical `detail`) |
|---|---|
| `disk_full` | A filesystem ran out of space or inodes (`/workspace: 0 bytes free`) |
| `oom` | A process was killed for exceeding its memory limit (`cargo killed by SIGKILL, oom_kill=1`) |
| `toolchain_missing` | A tool the step needs is not installed (`cargo: command not found`) |
| `network` | DNS, connect or timeout failure (`could not resolve github.com`) |
| `load` | The node was too loaded to finish in time (`load average 40 on 4 CPUs`) |
| `unknown` | A kind this consumer does not know; treat it as a generic environment failure. Decode-only: never sent |

In TypeScript the decoder passes the wire string through; use `environmentKind()` to map anything outside the known set to `"unknown"`.

sandbox-init currently emits `disk_full` (ENOSPC in the step output, or no free blocks/inodes in the workspace) and `oom` (the cgroup `oom_kill` counter rose during a failed step we did not stop ourselves); code failures, successes and interrupts carry none.

`StepGit`: `{repo, branch, head_sha, pushed}`, one per write workspace in `step_start.workspaces` order. The engine re-verifies the git facts. A missing required output is left out (the engine reports it).

`Usage` (on `StepFinished`, omitted when the agent reported none; old decoders ignore it):
`{input_tokens?, output_tokens?, cache_read_input_tokens?, cache_creation_input_tokens?: integer, models?: string[], cost_usd?: number}`.
Every field is omitted when unknown: absent means "not reported", never 0. `cost_usd` is the cost as reported by the agent (Claude's `total_cost_usd`), never an estimate computed by the sandbox.

### Error codes

`unsupported_version`, `unknown_message`, `protocol_violation`, `flow_control`, `unauthorized`, `unknown_channel`, `unknown_run`, `internal`, `unimplemented` (peer lacks the feature, e.g. `step_*` without a step executor). Unknown codes must be tolerated.

### `welcome.features`, `open.repo`/`resume`, `open_error.code`

Additive fields (protocol version stays 1). Encoders omit them when absent; `welcome.features` is also omitted when empty. Old peers ignore them and old payloads decode without them.

- `hello.features`: feature names the client supports, sanitized like `welcome.features`. Defined: `claude_user_token` (sent only by sandbox-proxy: it stores `credential_captured.token`, never relays it to a browser, and acks `legacy` captures with `credential_state`). A server sends token-bearing `credential_captured` frames **only** to a client that offered it; old clients get the token-less frame. Like `client`, it is self-declared and trusted only as far as the bearer auth.
- `welcome.features`: feature names the server supports. Defined: `acp` (the `acp` channel kind), `stats` (the `stats_request`/`stats` frames; a sandbox without it answers `stats_request` with `unknown_message`, so clients wait for the feature), `steps` (the `step_*` and `session_event` frames). Decoders drop non-string, empty and longer-than-64-byte entries and keep at most 64. Unknown features are ignored. A client sends something that depends on a feature **only after** `features` contains it.
- `open.kind`: `pty` or `acp`. A decoder maps any other kind to an "unknown" value instead of failing; a server answers it with `open_error` (`unsupported_kind`) and keeps the connection. For `acp`, `cols`/`rows` are ignored but clients send `0`, and `command` is not used.
- `open.repo` (at most 2048 bytes) and `open.resume` (at most 256 bytes) are `acp` routing hints. Longer values are refused with `open_error` (`invalid_request`) before the application sees them.
- `open_error.code`: machine-readable reason. Known: `chat_disabled`, `chat_limit`, `no_credential`, `repo_not_cloned`, `unsupported_kind`, `invalid_request`. Unknown codes must be tolerated; `message` stays the human text.

**Hazard: old proxies.** A proxy built before the `acp` kind decodes every browser frame and closes the whole websocket (1008) on a frame that fails to decode, which includes `open` with an unknown `kind`. That would also kill the terminal on the same socket. A sender must therefore not send `open{kind:"acp"}` unless the `welcome` it received listed `"acp"` in `features`; an old proxy never sends `features`. A proxy that forwards `welcome` should advertise the intersection of the sandbox's features and the ones it knows how to relay.

## 6. Handshake and version negotiation

1. The client's first frame must be `CTRL hello`. Anything else: `error` (`protocol_violation`) and disconnect.
2. The server picks the **highest version present in both** `versions` and its supported list and replies `welcome {version, features?}`.
3. If there is no common version the server sends `error` (`unsupported_version`) and closes the websocket.
4. No other frames (except `error`) are valid before `welcome`. A second `hello` is a violation.
5. Version 1 is described here. Later versions are additive where possible; behavior differing by version is keyed on the negotiated value.

`sandbox-init` authenticates the websocket upgrade (bearer token) before the handshake; the protocol itself carries no credentials.

## 7. Limits

- Max payload: 1,048,576 bytes. Larger length fields are rejected on seeing the header, before buffering the payload.
- Initial window: 262,144 bytes per channel per direction.
- Max single `WINDOW` grant and max accumulated credit: 2^31-1.

## 8. Errors

Decoding errors (stable `code` strings, used by `vectors/errors.json`): `truncated`, `trailing_bytes`, `unknown_type`, `unknown_flags`, `payload_too_large`, `ctrl_channel`, `zero_channel`, `empty_data`, `bad_window_payload`, `bad_credit`, `bad_close_payload`. They are connection-fatal: best effort `error` (`protocol_violation`) then drop the websocket. Application-level problems (bad channel, unknown run) use `error` or `open_error` and leave the connection up.

## 9. Test vectors

`vectors/*.json`, shared by the Rust and TypeScript tests:

- `frames.json`: `{name, hex, frame: {type, flags, channel, payload_hex}}`.
- `ctrl.json`: `{name, hex, message}`. `hex` is the whole frame as the Rust encoder emits it (compact JSON, struct field order). A decoder must parse `hex` to `message`; an encoder in another language should compare the *parsed JSON* of the payload, not the raw bytes, unless it matches the field order.
- `json_types.json`: `{name, type, json}` for non-CTRL bodies (`RunFinished`).
- `errors.json`: `{name, hex, error}`: decoding must fail with that error code.

Regenerate with `ENZARB_UPDATE_VECTORS=1 cargo test -p enzarb-proto --test vectors`.

## 10. Reference session runtime

`enzarb_proto::session` (Rust, feature `tokio`) and `Session` in `@enzarb/proto` (TS, client role) implement the rules above on top of a websocket. Behavior worth knowing when writing another implementation:

- **Fatal for the connection:** frame decode errors, a text websocket message, a bad handshake. The runtime sends `error` (`protocol_violation`) best effort and drops the websocket.
- **Per-channel violations** (credit overrun, `WINDOW` overflow, data after the peer's `CLOSE`): `error` (`flow_control`), `CLOSE` code 1 on that channel, and that channel is failed. The connection stays up.
- **Non-fatal replies:** data for an unknown channel -> `error` (`unknown_channel`); `resize` for an unknown channel -> `error` (`unknown_channel`); unknown CTRL `type` -> `error` (`unknown_message`); a second `hello`, or a message sent in the wrong direction -> `error` (`protocol_violation`).
- **`open`:** channel 0 or an id in use (including one with a pending `open`) is answered with `open_error`. The server's handler may accept (`opened`) or refuse (`open_error`).
- **Reply correlation:** replies carry no request id except by `channel` (`opened`/`open_error`) and `run_id` (`step_accepted`, `step_state`). An `error` with `run_id` is attributed to the client's pending `step_start`/`step_status` for that run (any code; if none matches it is delivered as an event). Without `run_id`, an `error` with code `unknown_run`, `internal`, `unauthorized`, `unknown_message` or `unimplemented` is attributed to the oldest pending request; other error codes are delivered as unsolicited events.
- **Credit:** a receiver returns credit when the application *reads* data, batched at half the initial window. If the application drops a channel's receiver, further data is discarded and credited immediately so the peer does not stall.
- **DATA_JSON** frames cannot be split, so the runtime refuses to send one larger than the initial window (256 KiB).
- Dropping the last session handle (or the sender of a channel) closes gracefully: `CLOSE` going-away on every open channel, then the websocket.

## 11. Open points

Reconnect/resume of channels is out of scope for version 1 (see ADR-0003 and gaps.md).

## 12. The `acp` channel kind

An `acp` channel carries the [Agent Client Protocol](https://agentclientprotocol.com)'s native stdio framing: newline-delimited JSON-RPC (one JSON object per line, UTF-8, `\n` terminated) as an ordinary **byte stream on `DATA_BIN`**, using the normal per-channel credit window in both directions.

- **Not one message per frame.** A single JSON-RPC message (tool output, a diff, replayed history) can be larger than the 256 KiB window or the 1 MiB frame cap, and `DATA_JSON` frames cannot be split. The stream may be split at any byte, including inside a line or a UTF-8 sequence; receivers buffer until `\n`. Senders should still prefer frames <= 16 KiB.
- **Attacker-controlled input.** The protocol layer does not parse the stream. The endpoint that does (sandbox-init for chat) must cap the length of a single line (the chat design uses 4 MiB) and close with reason `limit` when it is exceeded, and must treat malformed JSON as that channel's problem only. The mux guarantees the frame cap (1 MiB), the credit window and the `repo`/`resume` limits above.
- **Flow control.** The `@enzarb/proto` `openAcp()` readable pulls from the channel only when its consumer reads (high-water mark 0) and returns credit as it does, so a slow reader stalls the peer's writes end to end. Writes are sequential, so chunk order is preserved.
- **Close.** `CLOSE` is a half-close as for any channel. Closing from the client side ends the agent. Server reasons: `idle`, `superseded`, `shutdown`, `limit`, `exit:<n>`, `signal:<n>`; tolerate others.
- **Availability.** Use only after `welcome.features` contains `acp` (see the hazard above).
