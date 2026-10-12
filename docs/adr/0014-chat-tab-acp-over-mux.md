# 0014. Interactive chat over ACP, carried on a new mux channel kind

Extends [ADR-0011](0011-acp-agent-runs.md) (agent runs through ACP) and [ADR-0003](0003-mux-protocol.md) (mux protocol). Full design: [docs/design/chat-tab.md](../design/chat-tab.md) (enzarb-ui#66).

## Context
The UI needs a Chat page where the Project owner talks to Claude inside the Project's sandbox. Agent runs (ADR-0011) are non-interactive: sandbox-init is the ACP client, offers no client capabilities and answers every `session/request_permission` itself with `Policy::decide`. The mux has one channel kind (`pty`), `welcome` carries only `version`, and the sandbox-proxy decodes every browser frame, so a frame it cannot decode closes the socket with 1008. Chat must therefore be added without breaking old proxies, old sandbox images or old UIs.

## Decision
- **Transport:** a new mux channel kind `acp`. DATA_BIN carries ACP's native newline-delimited JSON-RPC (NDJSON) byte stream, so a message may span frames and the existing credit-based flow control applies. Additive wire changes, protocol version stays 1: `welcome.features` (optional list), `open.kind: "acp"` with optional `repo` and `resume`, `open_error.code`, CLOSE reasons `idle | superseded | shutdown | limit` (plus `exit:<n>` / `signal:<n>`), and `ChannelKind::Unknown` so a future kind yields `open_error{code:"unsupported_kind"}` instead of a decode failure.
- **Compatibility rule:** a sender must not send `open{kind:"acp"}` before it has seen `"acp"` in `welcome.features`. The proxy forwards the sandbox's features intersected with the ones it can guard; an old proxy or sandbox sends none, and the UI shows why chat is unavailable.
- **sandbox-init is a filtering relay, not a pipe.** It spawns one `claude-agent-acp` child per `acp` channel and parses every line in both directions: method allowlist, `clientCapabilities` forced to none, `cwd` forced to the chat worktree, `mcpServers` forced to `[]`, `session/load` only for sessions in its registry, `session/set_mode` limited to `default` and `plan`, the Claude token redacted, and every message appended to a transcript. The proxy stays protocol-agnostic above the frame level (it relays `acp` DATA byte for byte).
- **Two-tier permissions.** (1) Hard deny in the sandbox (`Policy::hard_deny`): secrets, `/workspace/.enzarb/**`, the mux auth token, edits outside the chat worktree, `git push`, fetches to non-allowlisted hosts. These are answered `reject_once` by sandbox-init and reported to the browser as `_enzarb/permission_denied`. (2) Everything else goes to the user, with the sandbox's opinion attached as a hint (`_meta.enzarb`), never as a default. Only `allow_once`/`reject_once`/`cancelled` responses are accepted; `*_always`, unknown ids and malformed answers become `reject_once`. Nothing is auto-approved, and a timeout, close or idle resolves to `cancelled`, never to allow.
- **Own chat cap.** `ENZARB_MAX_CHATS` (sandbox-init config only; default 1, `0` disables and removes the `acp` feature). Chat does not consume `ENZARB_MAX_RUNS` slots and does not block drain. At the cap, a new open closes the existing chat with `superseded` (last tab wins). No Project spec field in v1, so the operator is unchanged.
- **No browser fs or terminal capabilities.** The agent uses its own built-in tools inside the sandbox; `fs/*` and `terminal/*` requests from the agent get `-32601`.
- **No push, deps or npm tokens for chat.** The chat agent gets only the Claude token, so it cannot push (structurally, as well as by policy) and no new credential reaches the browser. Auth is the terminal's path unchanged (5-minute JWT, owner check in the proxy).
- **Storage:** a worktree per chat at `/workspace/worktrees/chat-<chat_id>` (local branch `enzarb/chat-<chat_id>`), registry and transcripts under `/workspace/.enzarb/chats/<chat_id>/`, and `CLAUDE_CONFIG_DIR` on the PVC so `session/load` survives pod restarts.
- **Rollout:** proto, then sandbox-container, sandbox-proxy, ui, helm. The UI is behind `ENZARB_UI_CHAT` (chart `ui.chat.enabled`, default false).

### Decisions on the open questions
- **Q1 (shared Claude quota): accepted.** Chat and jobs share the owner's Claude subscription and rate limits, so chat can contend with jobs. Revisit if it bites.
- **Q2 (private packages in chat): no npm or deps token for chat in v1.** `npm ci` of private `@enzarb/*` packages fails in a chat worktree; the executor's long-lived npm token is not made available to the sandbox for chat.
- **Q3 (pushing from chat): deferred.** A "push branch / open PR from this chat" action is a later feature, routed through workflows with a fresh `clone_token`, not through the sandbox.

## Consequences
- Old UIs, proxies and sandbox images keep working: no `acp` open is sent without the feature, so there is no 1008 surprise on the terminal path. Wire changes are additive, and shared vectors cover them.
- The security boundary is sandbox-init's relay (high-risk code with its own e2e tests using a fake ACP agent); the proxy and the browser are not trusted to filter.
- Chat shares sandbox CPU and memory (about +0.5 to 1 GiB per live chat agent on top of the run agents); no new knobs.
- A pod roll interrupts an in-flight turn (the UI reconnects and loads the session); chat does not hold up drain.
- The proxy `idle_timeout` default must rise to at least `ENZARB_CHAT_IDLE_SECS` (35 minutes vs 30).
- Chat cannot push or install private packages until the deferred features exist.
- A future cookie/Origin auth for `/sandbox/` (see enzarb-realtime#8, enzarb-ui#71) is a follow-up; until then chat uses the query token like the terminal.

## Alternatives rejected
- **One JSON message per DATA_JSON frame:** a single message (tool output, diff, replayed history) can exceed the 256 KiB credit window or the 1 MiB frame cap. A byte stream with NDJSON framing is what ACP's stdio transport already is, and `ndJsonStream` consumes it directly.
- **Parsing a command out of a PTY channel:** a PTY echoes and applies line discipline, with no framing guarantee, and would expose a shell to the chat path.
- **Exposing the agent directly (transparent pipe, browser as the ACP client with its own capabilities):** the browser would control `cwd`, MCP servers, modes and capabilities, and could answer permissions with `allow_always`. The sandbox must enforce these, so it filters.
- **Auto-approving what `Policy::decide` allows:** acceptable for unattended runs, but in chat the user is present and prompt injection from repo content is the risk, so only hard denies are automatic.
