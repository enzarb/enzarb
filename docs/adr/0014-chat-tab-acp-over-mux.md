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
- **Rollout:** proto, then sandbox-container, sandbox-proxy, ui, helm. The UI was initially behind `ENZARB_UI_CHAT` (chart `ui.chat.enabled`, default false); see the amendment below.

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

## Amendment 2026-10-12

The UI flag was removed after validation (ui v0.16.0, enzarb-helm v0.56.0; the chart keeps `ui.chat.enabled` only as a deprecated no-op schema key for one release). Chat is always on: the Chat tab is always listed and degrades to "Chat not available" when the sandbox or proxy does not advertise `acp`. The kill switch is `ENZARB_MAX_CHATS=0` on the sandbox (no `acp` feature, `chat_disabled` open errors). Rollback is a drain-first sandbox image rollback or that setting, not a UI flag.

## Amendment 2026-10-12 (2): chat default permission mode is bypass

Maintainer decision, shipped in enzarb-sandbox-container#51 (v0.9.0, issue #50). This amends the "Two-tier permissions" and `session/set_mode` decisions above; those texts stay as written.

**Decision.** A chat session starts in Claude Code's `bypassPermissions` mode. Chat runs inside the sandbox, and the sandbox, not a per-call prompt, is the security boundary. Agent jobs are unchanged: they keep their strict `Policy::decide` policy.

**What changes.**
- No permission prompts in bypass: Claude Code asks for nothing except its own bypass-immune checks. `Policy::hard_deny` (S1) is consulted only in `default` and `plan` mode, because the agent does not send `session/request_permission` for ordinary tool calls under bypass. There is no hard deny for ordinary tool calls in bypass, so the hard-deny rules (secret paths, `.enzarb/**`, `git push`, fetch hosts) do not apply there.
- `ENZARB_CHAT_PERMISSION_MODE` (`bypassPermissions` default, `default`, `plan`; any other value fails startup) and `ENZARB_CHAT_ALLOW_BYPASS` (default true). With `ENZARB_CHAT_ALLOW_BYPASS=false` the default mode becomes `default`, and `false` combined with an explicit bypass mode fails startup.
- The `session/set_mode` allowlist is {`bypassPermissions` (only while `ENZARB_CHAT_ALLOW_BYPASS` is true), `default`, `plan`}. `acceptEdits`, `auto` and `dontAsk` stay refused.
- sandbox-init enforces the mode itself: the adapter (claude-agent-acp 0.88) ignores `_meta.claudeCode.options.permissionMode` and takes the mode from the settings' `defaultMode` or a resumed transcript. When `modes.currentModeId` in the `session/new`/`load` response differs from the configured mode, sandbox-init sends its own `session/set_mode` and holds the browser's response until it succeeds (failure: error and no session).
- New browser notification `_enzarb/mode {sessionId, modeId, bypassAllowed}`, sent after the session is established and after each successful browser `set_mode`. Mode changes made by the agent (leaving plan mode, which can only go to `default`) arrive as `session/update` `current_mode_update`.
- The UI mode indicator and switch are not shipped yet; tracked as enzarb/enzarb-ui#87. Until then the UI neither shows that a chat is in bypass nor can switch the mode.

**What still bounds a prompt-injected chat agent.** No clone, push, deps or npm token in its scrubbed environment; cwd forced to the chat worktree; no MCP servers and no client fs/terminal capabilities; gVisor and the egress NetworkPolicy; line, idle and lifetime limits; redaction of the Claude token (and its encodings) on all browser-bound traffic and transcripts; sandbox-init marks itself non-dumpable (`PR_SET_DUMPABLE(0)`) and `/workspace/.enzarb` is 0700 (token and callback files 0600), which keeps other uids out. The browser cannot reach `acceptEdits`/`auto`/`dontAsk`; the elevated ExitPlanMode options are `allow_always` and are rejected.

**Residual risks (as reported in the PR).** The agent runs as the same uid as sandbox-init and the jobs, so file modes separate nothing from it. A bypass chat agent can read the mux auth token file, `.enzarb/runs/*/callback.json` (callback bearer tokens), other chats' transcripts and `chat-home`, every clone, and `/proc/<pid>/environ` of a concurrently running job (its deps/npm token). Highest risk: it can write the shared clone's `.git/config` and `.git/hooks`, which sandbox-init's authenticated `fetch`/`push` for later jobs also uses (the clone/push token is in `http.extraheader`); a planted hook or config could capture that token and leak it, for example through the Claude API conversation. Follow-up needed: harden those git calls (hooksPath, fsmonitor, explicit URL, a separate bare repo or a separate uid). It can also plant hooks or allow rules in `chat-home/.claude/settings.json` that affect later chats (the mode itself is re-enforced). Not verified: whether gVisor enforces non-dumpable `/proc` access. An independent Opus review of the PR found no must-fix issues.

**Kill switches.** `ENZARB_CHAT_PERMISSION_MODE=default` (or `plan`) restores per-call prompts with S1 hard deny; `ENZARB_CHAT_ALLOW_BYPASS=false` makes bypass unavailable (not offered to the adapter, `set_mode` to it refused); `ENZARB_MAX_CHATS=0` disables chat. Agent jobs are unaffected by all three.

**Follow-up.** The git hook/config token-theft risk above is tracked as enzarb/enzarb-sandbox-container#52.

## Amendment 2026-10-13: shared uid accepted

enzarb/enzarb-sandbox-container#53 (v0.9.1) closed the git hook/config route (#52): authenticated git runs only in scratch bare repos with an empty environment and no hooks. The remaining same-uid exposure is **accepted**: while a job's authenticated git runs, a bypass chat agent can read its token from `/proc/<pid>/environ`; it can write clones, toolchains and `.enzarb/` that later jobs use; and it can read the mux token and run callback files. A separate uid in the same pod (enzarb/enzarb-sandbox-container#54) was declined. If chat needs stronger isolation later, the direction is a **chat-specific sandbox** (its own pod and volume, no job credentials), not a second uid in the shared pod.

> **Amended 2026-10-15 (ADR-0011 amendment): no permission policy.** `Policy::hard_deny`, `Policy::decide`, the `_meta.enzarb` hint and `_enzarb/permission_denied` are gone. sandbox-init allows every chat permission request itself (`allow_always` if offered, else `allow_once`), except leaving plan mode, which still goes to the user. The sandbox is the security boundary.
