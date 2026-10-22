# ACP support

`ez-acp` serves the agent over the Agent Client Protocol (v1, newline-delimited JSON-RPC 2.0).
`ez acp` serves on stdio; `ez` itself talks to an in-process agent over an in-memory byte pipe, so
the wire is the same in both cases.

Wire types are the official [`agent-client-protocol-schema`](https://crates.io/crates/agent-client-protocol-schema)
crate, pinned to `=1.12.0` with `default-features = false` (no `schemars`) and **no `unstable_*`
features**. `ez_acp::schema` re-exports its `v1` types plus our extension types and helpers.
The JSON-RPC layer (framing, request ids, response routing, error replies, the dispatch loop) is
the official [`agent-client-protocol`](https://crates.io/crates/agent-client-protocol) SDK (3.4,
`default-features = false`; it depends on the same `agent-client-protocol-schema =1.12.0`, so there
is one copy of the schema). `rpc.rs` is a thin adapter on top of it: a method-string + JSON `Value`
view (`Handler`, `Connection`) so `_ez/*` extensions, `_meta` and lenient parsing stay
independent of the typed schema, plus a tokio line transport. Request dispatch (`agent.rs`) is ours.

Transport behavior worth knowing:

- **Ordering**: the SDK runs notification callbacks inline in its dispatch loop and routes responses
  in the same loop, so `session/update` notifications sent before a response are handled by the
  receiver before that response resolves (regression test: `rpc::tests::response_after_all_prior_notifications_handled`).
  A notification handler must not await a request on the same connection.
- **Concurrency**: each request runs on its own tokio task, so a long `session/prompt` can issue
  `session/request_permission` / `_ez/ask_user_question` and still receive `session/cancel`.
- Blank and non-JSON lines on the wire are ignored (the SDK itself would answer with `-32700`).
- A request whose `initialize` lacks a valid `protocolVersion`, or asks for a version above 1, is
  clamped to 1 before the SDK sees it (its version guard would otherwise reject the former and drop
  capabilities of the latter); the response still negotiates `min(requested, 1)`.
- Per JSON-RPC, `params` must be an object or array (the SDK rejects scalars with `-32602`); all ACP
  and `_ez/*` params are objects.

Inbound parsing is lenient where the official types are strict, so lenient clients keep working:
a missing `protocolVersion` means v1; `mcpServers` may be omitted (and a malformed or unsupported
entry is skipped, not fatal); prompt content blocks of unknown type are skipped (logged at debug)
instead of failing the prompt.

## Capabilities advertised by `initialize`

```json
{"protocolVersion": 1,
 "agentCapabilities": {
   "loadSession": true,
   "promptCapabilities": {"image": false, "audio": false, "embeddedContext": true},
   "mcpCapabilities": {"http": true, "sse": false},
   "sessionCapabilities": {"list": {}, "resume": {}, "close": {}}},
 "authMethods": [], "agentInfo": {"name": "ez", "version": "..."}}
```

Client capabilities we look at: `session.notices` (standard `notice` updates, see below),
`_meta.ez.askUserQuestion` and `_meta.ez.notices` (extensions). `ez` sets all of them.

## Methods (client -> agent)

| Method | Notes |
| --- | --- |
| `initialize` | Negotiates `protocolVersion` 1 (`min(requested, 1)`). |
| `authenticate` | No-op, returns `{}` (no auth methods are advertised). |
| `session/new` | `{cwd, mcpServers?}` -> `{sessionId, modes, configOptions}`. See [MCP servers](#mcp-servers). |
| `session/load` | `{sessionId, cwd, mcpServers?}`. Replays history as `session/update` notifications, then returns `{modes, configOptions}`. |
| `session/resume` | Same as `session/load` but without replaying history. |
| `session/list` | `{cwd?, cursor?}` -> `{sessions: [{sessionId, cwd, title, updatedAt, _meta}]}`, newest first, never paginated. `title` is the first line of the first prompt; `_meta.ez` has `createdAt`, `mode`, `firstPrompt`. Sessions are stored per project, so a missing `cwd` lists the agent process's working directory. |
| `session/close` | Cancels any running prompt and drops the session's in-memory state (MCP connections included). The transcript stays on disk; `session/load`/`resume` can reopen it. |
| `session/prompt` | Returns `{stopReason}` (`end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`). |
| `session/cancel` | Notification. |
| `session/set_mode` | Emits `current_mode_update`. |
| `session/set_config_option` | Only the `model` option: `{sessionId, configId: "model", value: "<model id>"}` -> `{configOptions}`. Pins the model for the session like `_ez/set_model`. |

Not implemented: `logout`, `session/delete`, `session/fork`, `fs/*` and `terminal/*` (the agent never
calls the client for files), `elicitation/*`. Unknown methods (including unknown `_`-prefixed ones)
fail with `-32601`.

### Config options

`session/new`, `session/load` and `session/resume` return `configOptions` with one select:

```json
{"id": "model", "name": "Model", "category": "model", "type": "select",
 "currentValue": "claude-opus-5", "options": [{"value": "claude-opus-5", "name": "claude-opus-5"}]}
```

`options` are the session profile's `model` plus its `models`; `currentValue` is the effective model
for the current mode. Whenever the effective model changes (pin, or a mode switch that changes the
model) the agent sends a `config_option_update` with the new state.

## Agent -> client

- `session/update`: `user_message_chunk` (replay), `agent_message_chunk`, `agent_thought_chunk`,
  `tool_call`, `tool_call_update` (text content plus one `diff` per edited file; `content` is
  omitted when there is nothing new, because a present `content` replaces what the client shows),
  `plan`, `current_mode_update`, `config_option_update`, `usage_update`, `notice`.
  - `usage_update`: `{used, size}` after every model request, where `used` is the provider-reported
    input plus output tokens and `size` the profile's `context_window`. Not sent when the provider
    reports no usage.
  - Notices (hook output, model changes, MCP start-up failures, ...) are sent as the standard
    `notice` update (`severity: "info"`, `title`) only if the client advertised
    `clientCapabilities.session.notices` (or `_meta.ez.notices = true`). Otherwise they stay
    `agent_message_chunk` text `"\n> {notice}\n"`.
- `session/request_permission`: options `allow_once` ("Allow"), `allow_always` ("Always allow",
  only when offered), `reject_once` ("Reject"). A cancelled outcome or RPC error cancels the turn.

Wire notes (the official serializer's conventions): `tool_call` omits `status` when `pending` and
`kind` when `other`; diffs of new files omit `oldText`.

## Extensions

### `_ez/ask_user_question` (agent -> client)

Used only if the client sets `clientCapabilities._meta.ez.askUserQuestion = true` in `initialize`.

```json
{"sessionId": "...", "questions": [{"question": "Which DB?", "header": "DB", "multiSelect": false,
  "options": [{"label": "sqlite", "description": ""}]}]}
```
Result: `{"answers": [{"question": "Which DB?", "selected": ["sqlite"], "other": null}], "cancelled": false}`.
`cancelled: true` fails the tool call.

Without the capability, each question is asked via `session/request_permission`, with one
`allow_once` option per choice (`optionId` `opt-<index>`, `name` the label); free text is unavailable.

### `_ez/list_sessions` (client -> agent) - deprecated

Deprecated alias of the standard `session/list`, kept for existing clients.
`{"cwd": "/project"}` -> `{"sessions": [{"id", "createdAt", "updatedAt", "firstPrompt", "mode"}]}`,
newest first. `ez` itself uses `session/list`.

### `_ez/set_model` (client -> agent)

`{"sessionId": "...", "model": "claude-haiku-5-5"}` -> `{}`. Pins the model for the session,
overriding the mode's and profile's model (recorded in the transcript as a `model_change` record and
announced as a notice). `model: null` or absent clears the pin, which the standard
`session/set_config_option` cannot express; used by the TUI's `/model`. Selecting a model with
`session/set_config_option` is equivalent to a non-null pin.

## MCP servers

`mcpServers` on `session/new`, `session/load` and `session/resume` are added to that session
(overriding same-named servers from config) and connect on the first prompt:

```json
[{"name": "fs", "command": "/usr/bin/mcp-fs", "args": ["--root", "."], "env": [{"name": "K", "value": "v"}]},
 {"type": "http", "name": "docs", "url": "https://example.com/mcp", "headers": [{"name": "Authorization", "value": "Bearer t"}]}]
```

SSE entries are ignored (`mcpCapabilities.sse` is false). Which tools a mode sees is still governed by
its `mcp_tools` selectors (the implicit default mode selects all). Start-up failures arrive as a
notice. MCP tool calls appear as ordinary `tool_call`s titled
`<server>: <tool>` with kind `read` (read-only annotated) or `other`, and ask via
`session/request_permission` unless allowed by `[permissions]`.

## Modes

Lifecycle modes (from `.ez/modes.toml`) map to ACP session modes: `id`/`name`/`description`
per mode, `currentModeId` is the active mode. The agent may switch modes itself (`SwitchMode` tool);
that also arrives as `current_mode_update`.

## Compatibility with the previous hand-written schema

Intended wire differences, all covered by `crates/ez-acp/tests/golden.rs` (baseline captured
before the migration): `sessionCapabilities`/`auth` in the `initialize` response; `configOptions` in
session responses; `tool_call_update` without `content` instead of `content: []`; diffs without
`"oldText": null`; `tool_call` without default `status`/`kind`; new `usage_update`,
`config_option_update` and (for capable clients) `notice` updates.
