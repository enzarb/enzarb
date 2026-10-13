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
- `run_start.log_channel`, when non-null, is a client-chosen id on which the server streams the run transcript as `DATA_JSON` frames. It is implicitly opened by `run_accepted` and uses the same flow control and close rules.

## 4. Flow control (credit based)

- Each data channel has an independent **window per direction**. Both start at `INITIAL_WINDOW` = 262,144 bytes (256 KiB) as soon as the channel is open (when the server's `opened` is sent/received, or for a log channel when `run_accepted` is).
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

JSON objects with a `"type"` field, channel 0. Unknown fields are ignored; an unknown `type` is answered with `error` (`unknown_message`). Optional fields may be absent or `null`; encoders omit `None` except `run_start.log_channel` which is always written (`null`). `run_start.deps_token` is an optional read-only (`contents: read`) token for fetching same-Project git dependencies; it is never used for push, and implementations must not log it. `run_start.npm_token` is an optional long-lived token with `read:packages` for installing private npm packages; implementations expose it to the run process only (as `NODE_AUTH_TOKEN`), must not log or persist it, and must redact it from transcripts. Definitions mirror contracts section 4 and `crates/enzarb-proto/src/ctrl.rs`.

| Message | Direction | Fields |
|---|---|---|
| `hello` | client -> server | `versions: [u16]`, `client: "ui" \| "proxy" \| "executor"` |
| `welcome` | server -> client | `version: u16`, `features?: [string]` |
| `open` | client -> server | `channel`, `kind: "pty" \| "acp"`, `cols`, `rows`, `command?: [string]`, `repo?: string`, `resume?: string`, `claude_token?: string` |
| `opened` | server -> client | `channel` |
| `open_error` | server -> client | `channel`, `message`, `code?: string` |
| `resize` | client -> server | `channel`, `cols`, `rows` |
| `run_start` | client -> server | `run_id, repo_url, base_ref, branch, push, clone_token, prompt, timeout_secs, callback_url, callback_token, log_channel, deps_token?, npm_token?, claude_token?` |
| `run_accepted` | server -> client | `run_id`, `state`, `existing: bool` |
| `run_status` | client -> server | `run_id` |
| `run_state` | server -> client (reply to `run_status`, and pushed on change) | `run_id`, `state`, `exit_code?`, `summary?` |
| `run_cancel` | client -> server | `run_id` |
| `credential_captured` | server -> client | `kind: "claude_oauth"`, `token?: string`, `legacy?: bool` |
| `credential_status` | client -> server | none; asks whether the sandbox holds a credential |
| `credential_state` | server -> client (reply to `credential_status`); client -> server (ack of a `legacy` `credential_captured`) | `kind: "claude_oauth"`, `configured: bool`, `scope?: string` (known: `"user"`; unknown values must be tolerated) |
| `credential_clear` | client -> server | `kind: "claude_oauth"`; no reply. A peer that does not know it answers `unknown_message` |

`claude_token` (on `run_start` and `open`) and `credential_captured.token` carry a per-user Claude token as a plain string. They are optional and additive (protocol version stays 1; old peers ignore them). Implementations must not log them: the Rust `Secret` type and the TS `Secret` class redact `Debug`/`inspect`/`String()`/`JSON.stringify`, and the wire encoder writes the plain value.
| `error` | both | `code`, `message`, optional `run_id`, optional `channel` (omitted when absent) |

`state` is `queued | running | succeeded | failed | cancelled`. `run_start` is idempotent on `run_id`.

### Run finished callback body (`RunFinished`)

Not a CTRL message; it is the JSON body sandbox-init POSTs to `callback_url`:
`{run_id, outcome: succeeded|failed|cancelled, exit_code?, summary, branch, head_sha, pushed}`.

### Error codes

`unsupported_version`, `unknown_message`, `protocol_violation`, `flow_control`, `unauthorized`, `unknown_channel`, `unknown_run`, `internal`, `unimplemented` (peer lacks the feature, e.g. `run_*` without a run manager). Unknown codes must be tolerated.

### `welcome.features`, `open.repo`/`resume`, `open_error.code`

Additive fields (protocol version stays 1). Encoders omit them when absent; `welcome.features` is also omitted when empty. Old peers ignore them and old payloads decode without them.

- `welcome.features`: feature names the server supports. Defined: `acp` (the `acp` channel kind). Decoders drop non-string, empty and longer-than-64-byte entries and keep at most 64. Unknown features are ignored. A client sends something that depends on a feature **only after** `features` contains it.
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
- **`run_start` with `log_channel`:** `run_accepted` is sent only after the channel is registered; if the id is in use the server answers with `error` instead and no channel is created.
- **Reply correlation:** replies carry no request id except by `channel` (`opened`/`open_error`) and `run_id` (`run_accepted`, `run_state`). An `error` with `run_id` is attributed to the client's pending `run_start`/`run_status` for that run (any code; if none matches it is delivered as an event). Without `run_id`, an `error` with code `unknown_run`, `internal`, `unauthorized`, `unknown_message` or `unimplemented` is attributed to the oldest pending `run_start`/`run_status`; other error codes are delivered as unsolicited events.
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
