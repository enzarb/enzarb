# Design: Project chat (enzarb-ui#66)

Status: proposed (2026-10-12). Scope: a Chat page in the project view that talks to Claude through ACP inside the Project's sandbox. It covers enzarb-proto, enzarb-sandbox-container, enzarb-sandbox-proxy, enzarb-ui, enzarb-helm and root docs. enzarb-workflows and enzarb-sandbox-operator need no code changes (see section 4).

This builds on enzarb's plan in the issue (separate routes under a shared layout, `@agentclientprotocol/sdk` rather than `@zed-industries/*`, no job-running notice) and settles the "to confirm" points.

## What exists today (facts this design relies on)

- **Agent runs** (ADR-0011): sandbox-init is an ACP *client* of `claude-agent-acp` (`ENZARB_ACP_AGENT_BIN`). It is spawned per run in its own process group (`enzarb-sandbox-container/src/acp.rs`), offers no client capabilities, runs in `/workspace/worktrees/run-<id>`, and gets `CLAUDE_CODE_OAUTH_TOKEN` from `/workspace/.enzarb/claude-token` (`agent.rs` ~l.1184). Every `session/request_permission` is answered by `Policy::decide` (`policy.rs`), with `allow_once`/`reject_once` only. A `Semaphore` of `ENZARB_MAX_RUNS` (default 2) limits concurrent runs.
- **Mux** (`enzarb-proto/docs/protocol.md`): `open` has `kind: "pty"` only. `ChannelKind` is a closed Rust enum with `Pty` only (`crates/enzarb-proto/src/ctrl.rs`). `welcome` carries only `version`. Frames are at most 1 MiB, and each channel has a 256 KiB credit window.
- **Proxy** (`enzarb-sandbox-proxy/src/relay.rs`, `guard.rs`): the proxy completes the sandbox handshake itself and answers the browser's `hello` locally with `welcome{version}`. It then decodes every browser frame and allowlists CTRL (`open`, `resize`, `error`, `credential_status`). **A frame that fails to decode closes the socket with 1008.** So an old proxy given `open{kind:"acp"}` kills the terminal-style connection. The proxy also has an `idle_timeout`.
- **UI**: `terminalConnection(name)` (`src/lib/terminal.remote.ts`) mints a 5-minute HS256 JWT (`sub`, `project`, `aud: enzarb-sandbox-proxy`) and returns `wss://<host>/sandbox/v1/projects/<p>/mux?token=…`. `Terminal.svelte` then runs `Session.connect(ws,{client:'ui'})` and `openPty`.
- **Drain** (`enzarb-workflows/crates/engine/src/executor/sandbox_change.rs`): the drain hold stops new job claims, and the drained annotation is set once no *job* is `running`. It knows nothing about interactive sessions.
- Claude's own session files live under `HOME=/home/enzarb`, which is **not** on the PVC (Dockerfile).

---

## 1. Transport

**Decision: a new mux channel kind `acp`, with DATA_BIN carrying ACP's native newline-delimited JSON-RPC (NDJSON) byte stream.** Advertising it is done with a new optional `welcome.features` list.

Why not ride an existing channel:
- **PTY:** it echoes, applies line discipline and has no framing guarantee. Rejected, as in the issue.
- **One DATA_JSON frame per JSON-RPC message:** rejected. A single message (tool output, diff, replayed history) can exceed the 256 KiB credit window or the 1 MiB frame cap, and the protocol requires senders to split data to fit credit. A byte stream with NDJSON framing is what ACP's stdio transport already is. It splits across frames freely, and `@agentclientprotocol/sdk`'s `ndJsonStream(writable, readable)` consumes it directly.

Wire changes (all additive, protocol version stays 1):

| Message | Change |
|---|---|
| `welcome` | `features?: [string]`. The sandbox sends `["acp"]` when chat is enabled (`ENZARB_MAX_CHATS > 0`). Absent means none. |
| `open` | `kind: "pty" \| "acp"`. For `acp`, `cols`/`rows` are ignored, but clients send `0` so older decoders still parse. New optional `repo?: string` (a repo URL of the Project) and `resume?: string` (ACP session id, informational: the routing hint for the cwd lookup). |
| `open_error` | optional `code?: string`: `chat_disabled`, `chat_limit`, `no_credential`, `repo_not_cloned`, `unsupported_kind`. |
| `CLOSE` reason on an `acp` channel | `idle`, `superseded`, `shutdown`, `limit`, `exit:<n>`/`signal:<n>`. Clients must tolerate other text, as today. |
| `ChannelKind` (Rust) | add `Acp` and a `#[serde(other)] Unknown`. A future kind then yields `open_error{code:"unsupported_kind"}` instead of a decode failure. |

**Capability negotiation and version skew.** The UI sends `open{kind:"acp"}` **only if** `welcome.features` contains `"acp"`.
- The proxy builds the browser's `welcome` itself, so it **forwards the sandbox's features intersected with the features it knows how to guard**.
- Old proxy (any sandbox): it sends no `features`, so the UI shows "Chat is not available yet: the sandbox gateway needs an update". The UI never sends a frame the old proxy would 1008 on.
- New proxy, old sandbox image: the sandbox sends no `features`, so the UI shows "This sandbox image does not support chat. It updates on the next sandbox restart".
- New proxy, new sandbox, `ENZARB_MAX_CHATS=0`: no feature, so the UI shows "Chat is disabled for this sandbox".
- Old UI: it never opens `acp` and ignores the unknown `features` field (`Ctrl` decode ignores unknown fields).

**TS package** (`enzarb-proto/ts/src/session.ts`, `channel.ts`, `ctrl.ts`):
- `Session.features: ReadonlySet<string>`.
- `Session.openAcp({repo?, resume?}): Promise<ByteChannel>`, where `ByteChannel` exposes `readable: ReadableStream<Uint8Array>`, `writable: WritableStream<Uint8Array>` and `closed: Promise<{reason}>`. Credit is granted as the readable is consumed, so backpressure flows end to end.
- `OpenError` carries `code`.

**Rust crate:** `Session::open_acp` for tests and the proxy, `Welcome.features`, the `OpenError.code` field, and `ChannelKind::{Acp, Unknown}`. Add test vectors for the new shapes in `vectors/`.

## 2. Where the agent lives, and its lifecycle

**Decision: sandbox-init spawns one `claude-agent-acp` child per `acp` channel**, reusing `acp::spawn` (own session/process group, reaper-owned). sandbox-init is **not** a transparent pipe. It is a filtering JSON-RPC relay between the channel and the child's stdio (new module `src/chat.rs`, sharing `acp.rs`'s spawn/terminate helpers). It parses each NDJSON line in both directions. The max line is 4 MiB; a longer line closes the channel with `limit`.

**Working directory.**
- Each chat gets its own worktree, `/workspace/worktrees/chat-<chat_id>`. `chat_id` is a random 16-hex id that sandbox-init generates on `session/new`.
- The worktree is detached at `origin/<default branch>` of the chosen clone. The clone is `open.repo`, which must already exist under `/workspace/repos/`; otherwise the open fails with `repo_not_cloned`. Without `repo`, the first existing clone is used.
- If the agent commits, it commits on the local branch `enzarb/chat-<chat_id>`.
- Run pruning (`prune_worktrees`) only touches `run-*`, which is already the case. Add a `chat-*` sweep that removes chat worktrees with no live channel and no activity for 7 days (`ENZARB_CHAT_RETENTION_DAYS`).
- Registry: `/workspace/.enzarb/chats/<chat_id>/{meta.json, transcript.jsonl}`. `meta.json` holds `{acp_session_id, repo, worktree, created, last_active}`.

**Environment.**
- The child gets the same base env as runs (toolchain paths, `PATH`) plus `CLAUDE_CODE_OAUTH_TOKEN` read from the token file at spawn time.
- Set `CLAUDE_CONFIG_DIR=/workspace/.enzarb/chat-home/.claude` so Claude's session files persist on the PVC, which `session/load` across pod restarts needs. Verify the env name against the pinned adapter version.
- **No** `clone_token`, `deps_token` or `npm_token`. Those exist only inside a run. So the chat agent **cannot push** (structurally, not only by policy), and it cannot `npm ci` private `@enzarb/*` packages (open question Q2).
- No new credential reaches the browser.

**Rewrites applied by sandbox-init.**

Browser to agent (method allowlist; anything else gets JSON-RPC `-32601`):
- `initialize`: `clientCapabilities` forced to `{fs:{readTextFile:false,writeTextFile:false}, terminal:false}`. `clientInfo` passes through.
- `session/new`: `cwd` forced to the chat worktree and `mcpServers` forced to `[]`. On the response, record `sessionId` → `chat_id`.
- `session/load`: only for a `sessionId` present in the registry (else `-32602`). `cwd` is forced to that chat's worktree and `mcpServers` to `[]`. Forward only if the agent's `initialize` response advertised `agentCapabilities.loadSession`; else `-32601`.
- `session/prompt`: max 256 KiB of text blocks. Only `text` and `resource_link` blocks pass in v1.
- `session/cancel`.
- `session/set_mode`: only `default` and `plan`. `acceptEdits` and `bypassPermissions` are refused. This is the main way to bypass per-call approval, so it is blocked.
- JSON-RPC **responses** are accepted only for an outstanding forwarded `session/request_permission` id (see section 3). `authenticate` and all other methods are refused.

Agent to browser:
- `session/update` passes through after redacting the Claude token (and its forms) from all strings.
- `session/request_permission` goes through the policy flow (section 3).
- `fs/*` and `terminal/*` get `-32601` back to the agent. It should never send them, since the capabilities are false.
- Other agent-originated requests are refused. Notifications pass through.

Every message in both directions is appended, redacted, to the chat's `transcript.jsonl`, using the same record kinds as runs (`update`, `permission`, plus `prompt`). This gives an audit trail of what the agent was allowed to do.

**Lifecycle.**
- **Start:** on `open{kind:"acp"}`, sandbox-init checks the cap and the credential (`no_credential` if the token file is absent), spawns the child and replies `opened`. If the child exits before the browser's `initialize` is answered, sandbox-init sends `CLOSE exit:<n>`. The child's stderr goes to the transcript as `log{source:acp}`.
- **Stop (browser closes the channel or the websocket drops):** send `session/cancel` for active sessions, wait `cancel_grace` (as for runs), close stdin, then SIGTERM/SIGKILL the process group. A websocket drop goes through `session_closed`, already present in `agent.rs`, extended to the chat manager.
- **Idle:** if there is no browser→agent message and no agent output for `ENZARB_CHAT_IDLE_SECS` (default 1800), close with `idle`. A pending permission does **not** keep the session alive: it times out to `cancelled`, never to allowed.
- **Hard ceiling:** `ENZARB_CHAT_MAX_SECS` (default 8 h), then close with `limit`.
- **Shutdown:** on SIGTERM, sandbox-init sends `CLOSE shutdown` to every chat channel, then cancels and kills the children before the existing run handling.
- **Resume:** the UI keeps `{acp_session_id}` in `sessionStorage` under `enzarb.chat.<project>`. On reconnect it calls `session/load` if `loadSession` is advertised. The agent replays history as `session/update`, and the reducer rebuilds the transcript. If load is unsupported or fails, the UI starts `session/new` and shows "previous conversation not restored".
  - `claude-agent-acp` advertises `loadSession` in recent versions; check it at the pinned version.
  - Resume across a pod restart works only with the `CLAUDE_CONFIG_DIR` on the PVC described above.
- **Cancellation:** the Stop button sends `session/cancel`. The agent ends the turn with `stopReason: cancelled`, and pending permissions resolve `cancelled` (section 3).

## 3. Permissions and policy

> **Amended 2026-10-12 (ADR-0014 amendment 2): chat defaults to `bypassPermissions`.** The text below describes the `default`/`plan` modes, which stay available (`ENZARB_CHAT_PERMISSION_MODE=default|plan`, `ENZARB_CHAT_ALLOW_BYPASS=false`). In bypass the agent raises no `session/request_permission` for ordinary tool calls, so tier 1 (`Policy::hard_deny`) and tier 2 (user prompts) apply only in `default` and `plan`; the sandbox is the boundary. `session/set_mode` accepts `bypassPermissions` (only while allowed), `default` and `plan`; sandbox-init enforces the configured start mode itself (the adapter ignores `_meta` `permissionMode`) and notifies the browser with `_enzarb/mode {sessionId, modeId, bypassAllowed}`. Agent jobs are unchanged. The UI mode indicator/switch is not shipped (enzarb-ui#87).

**Decision: two tiers. Policy hard-denies first; everything else goes to the user; nothing is ever auto-approved.**

1. **Hard deny, in the sandbox.** A new `Policy::hard_deny(tool_call, worktree) -> Option<Decision>` (`policy.rs`) applies the non-negotiable subset of the existing rules, evaluated with `{worktree}` = the chat worktree:
   - reading `deny_read` paths (secrets), plus `/workspace/.enzarb/**` and `/var/run/enzarb/**` (the mux auth token);
   - edits outside `edit_roots`;
   - `git push` / `git remote set-url`, and `curl`/`wget` to hosts outside `fetch_allow_hosts` (`fetch` rule).

   On a hard deny, sandbox-init answers the agent `reject_once` itself. It sends the browser a notification, `_enzarb/permission_denied {sessionId, toolCall, rule, reason}`, using an ACP extension method (`_` prefix), so the UI shows "Blocked by sandbox policy: <reason>". The decision is logged.
2. **User decision, for everything else**, including commands that `Policy::decide` would allow and commands its parser cannot judge.
   - sandbox-init forwards the request with an added `_meta.enzarb = {policy: "allow" | "deny" | "unknown", rule, reason}` so the UI can show the sandbox's opinion as a hint, never as the default.
   - sandbox-init tracks the request id. When the browser responds, it checks that the response is `selected` with an `optionId` from that request's options whose `kind` is `allow_once` or `reject_once`, or `cancelled`. Any other answer (including `allow_always`/`reject_always`, unknown ids or malformed responses) is replaced by `reject_once`.
   - On channel close, idle or cancel, every outstanding request is answered `cancelled` to the agent.

What the agent asks about at all is decided by Claude's own permission mode (`default`; `set_mode` is restricted to `default`/`plan`). Reads inside cwd may not prompt, as in runs today. Hard-deny still applies to every request the agent does raise.

**Client capabilities:** none from the browser (section 2 forces this). The agent uses its own built-in tools inside the sandbox.

**The terminal comparison.** The Project owner already has a full shell in the same sandbox through the terminal. Chat does not give the *user* new power. The risk it adds is *agent-initiated* action, for example prompt injection from repo content. The user approval step and hard deny address exactly that.

## 4. Concurrency, resources, drain

Decisions:
- **Chat does not consume `maxRuns` slots.** It has its own cap, `ENZARB_MAX_CHATS` (default 1, `0` disables), set in sandbox-init config only. No Project spec field in v1, so the operator is unchanged; add `spec.maxChats` later only if someone asks.
- Projects have exactly one owner, and the proxy only admits `spec.owner`. So "1 per Project per user" is the same as "1 per sandbox".
- **Last tab wins.** An `open{kind:"acp"}` at the cap closes the existing chat channel with `superseded` and then opens the new one. This handles a second tab and stale half-open sockets without a lock-out. The superseded tab shows "Chat opened in another tab", with a "Take over" button that reconnects.
- **Working tree isolation:** the chat worktree is separate from run worktrees and the clone's checked-out branch. The chat branch name is `enzarb/chat-*`, which no workflow uses. Edits by a job and a chat never touch the same directory. Git operations on the shared clone go through the existing per-repo `repo_lock` when creating or removing chat worktrees.
- **No push:** there are no push credentials (section 2), and `git push` is hard-denied. Pushing a chat branch or opening a PR from chat is out of scope for v1.
- **Credential contention:** chat and jobs share the owner's Claude subscription and rate limits. See open question Q1.
- **Resources:** the chat child runs inside the sandbox container's existing CPU/memory limits (`spec.sandbox.cpu`/`memory`). Memory guidance in the docs: about +0.5 to 1 GiB per live chat agent on top of `maxRuns` agents. No new knobs.
- **Drain: chat does not block drain.** Interactive sessions can be open indefinitely, and the drain hold must not depend on a browser tab. Workflows and the operator stay unchanged.
  - Graceful handling: the layout already shows `PendingChangeBanner`. On the Chat page, it adds "The sandbox will restart to apply a change; this chat will reconnect afterwards."
  - On pod deletion, sandbox-init's SIGTERM path sends `CLOSE shutdown`. The UI shows "Sandbox restarting", reconnects with backoff, and `session/load`s the session, which persists on the PVC.
  - Work in the chat worktree survives on the PVC; only an in-flight turn is lost.

## 5. Auth and authz

- **Same path as the terminal.** The page calls a new remote function, `chatConnection(name)`, or reuses `terminalConnection` renamed to `sandboxConnection`; it keeps the same `ownedProject` check, JWT claims and URL. No new claims, route or secret. The proxy's owner check (contracts §7) is the authorization.
- **Proxy:** `guard.rs` allows `open` with `kind:"acp"`. DATA frames on that channel are relayed byte for byte, like PTY data. The sandbox does the JSON-RPC filtering, so the proxy stays protocol-agnostic above the frame level.
  - The proxy forwards `welcome.features` ∩ `{"acp"}`.
  - Its `idle_timeout` must be at least `ENZARB_CHAT_IDLE_SECS`, or chat will be cut while the user reads or thinks. Raise the default to 35 min and document it. WebSocket pings (proxy → browser, every 30 s) keep intermediaries open; add them if missing.
- **Cookie migration later:** enzarb-realtime#8 / enzarb-ui#71 are moving `/live` to an HttpOnly cookie with an Origin check. When that pattern settles, a follow-up issue in sandbox-proxy should accept the same cookie on `/sandbox/` upgrades: Origin must equal the public URL, and the `sub` and Project owner check stay the same. `?token=` is kept during a deprecation window. Chat needs no change because it shares the socket. Until then, chat uses the query token exactly like the terminal.
- **Abuse limits:**
  - 1 live chat per sandbox (cap), 1 `acp` channel per socket (sandbox-enforced);
  - 4 MiB max line, 256 KiB max prompt;
  - idle 30 min, hard 8 h;
  - method and mode allowlists.

  Prompts are not rate-limited: the cost falls on the owner's own Claude plan.

## 6. UI design (enzarb-ui)

Routes, as in enzarb's revised plan: `[name]/+layout.svelte` (header, `PendingChangeBanner`, polling, error states) plus child pages `+page.svelte` (Overview), `terminal/`, `chat/`, `settings/`. Navigation is `ProjectNav.svelte` with `<a aria-current>`. Legacy `?tab=` links get a 308 redirect.

Modules:
- `src/lib/acp/connection.ts`: wraps `@agentclientprotocol/sdk`'s `ClientSideConnection` over `ndJsonStream(ch.writable, ch.readable)`.
  - The `Client` implementation provides only `sessionUpdate`, `requestPermission` and `extNotification` (for `_enzarb/permission_denied`).
  - `readTextFile`/`writeTextFile`/terminal methods throw "not supported", and `initialize` advertises no capabilities.
  - If the SDK proves too heavy or Node-only in the browser, fall back to an in-repo JSON-RPC client that uses `import type` from the SDK only. Measure in PR 2.
- `src/lib/acp/reducer.ts`: pure `applyUpdate(state, update)`. Items: user/agent message chunks (merged), thoughts, tool calls (merged by `toolCallId`, with status/content/diff), plan, permission `{reqId, epoch, toolCall, options, policyHint, status: pending|allowed|denied|cancelled|blocked}`, and system notices (`resumed`, `not_restored`, `superseded`, `restarting`, `ended`).
- `src/lib/chat-steps.ts`: the connect checklist, with steps:
  1. Token
  2. Connect
  3. Handshake (`welcome`; fails here with the "unsupported" states in section 1)
  4. Start agent (`opened`/`open_error` code)
  5. Initialize
  6. Session (new / load)

  Each step is pending, active, done or failed with the sandbox's message. This uses `TerminalChecklist`'s pattern; no spinners. A running turn shows a "Working" item whose current step is the latest in-progress tool call title.
- Components in `src/lib/components/chat/`:
  - `MessageList`, `Message`, `ToolCall`, `PermissionRequest`, `Composer` (Enter sends, Shift+Enter newline, Stop while a turn runs), `ChatChecklist`, `ChatUnavailable` (the feature-skew messages);
  - `src/lib/components/Chat.svelte` owns connection, backoff and storage;
  - styles go in `global.scss` as `.ez-chat*`.
- `PermissionRequest`:
  - only "Allow once" and "Deny" buttons, which map to options of kind `allow_once`/`reject_once`;
  - shows the policy hint ("Sandbox policy: would deny — <rule>");
  - buttons are disabled after the first click;
  - an `epoch` (connection counter) check makes clicks from an earlier connection no-ops;
  - no keyboard default focus on Allow.
- Reconnect: on unexpected close, mark pending permissions `cancelled` and show "Disconnected". Reconnect automatically with capped exponential backoff (1 s → 30 s, jitter, max 6 tries), then offer a manual Reconnect.
  - Close reasons `superseded` and `limit` do **not** auto-reconnect.
  - `shutdown` and `idle` reconnect only when the page is visible (`visibilitychange`). For `idle`, the reconnect waits for the user's next action.
  - Each reconnect mints a fresh token.
- Gating: the Chat page requires `project.ready` and the Claude credential, using the existing `CredentialStatus` and the `/claude` link.
- Feature flag: `ENZARB_UI_CHAT` (server config, exposed through the layout load). When off, the nav item is hidden and `/chat` returns 404.

Tests (Vitest, `*.svelte.test.ts`):
- reducer: merging, tool-call lifecycle, and load-replay idempotence;
- permission safety:
  - nothing is sent without a click;
  - Allow/Deny pick the right option kind;
  - a double click sends once;
  - disconnect, unmount, reconnect and Stop cancel pending requests;
  - stale-epoch clicks are no-ops;
  - a grep-style test that no code path selects an option without a user event;
- client against an in-memory fake agent: no capabilities advertised, new/load fallback, prompt/cancel;
- checklist states for each `open_error` code and missing `features`;
- route/nav tests; a guard that there is no `@zed-industries/*` dependency.

## 7. Test strategy per repo

- **enzarb-proto:**
  - Rust and TS round-trip vectors for `welcome.features`, `open kind:"acp"` with and without `repo`/`resume`, `open_error.code`, and `ChannelKind::Unknown`;
  - a decode of an old `welcome` (no features) giving an empty set;
  - a session test: `openAcp` byte stream larger than the window (2 MiB) round-trips under flow control;
  - `cargo test`, `npm test`.
- **enzarb-sandbox-container:**
  - unit tests for the relay filter: `initialize` capability rewrite, cwd/mcpServers override, method and mode allowlists, permission response validation (always-options replaced, unknown id dropped), hard-deny paths, redaction, line cap;
  - `Policy::hard_deny` table tests with the chat worktree;
  - **end-to-end with a fake ACP agent:** a small test binary (`tests/fake_acp_agent.rs`, or a node script already in-image) set via `ENZARB_ACP_AGENT_BIN`. It advertises `loadSession`, streams scripted updates, issues `request_permission` for an allowed and a hard-denied command, and records what it received. The test drives sandbox-init's server over a real websocket with the Rust `Session::open_acp` and asserts:
    - forced capabilities and cwd;
    - the hard deny was answered without reaching the client;
    - the user answer passes through;
    - close → `session/cancel` → process group gone;
    - cap/supersede; idle close (short timeout); SIGTERM → `CLOSE shutdown`; `session/load` routing.
  - `cargo clippy --all-targets && cargo test`; `docker build .`
- **enzarb-sandbox-proxy:**
  - guard allows `open kind:acp` and still rejects `run_*`;
  - welcome features are intersected and forwarded;
  - an old-sandbox welcome (no features) gives none;
  - relay test with a large DATA_BIN stream on an acp channel;
  - idle timeout config.
- **enzarb-ui:** see section 6, plus one Playwright smoke test against a stub mux server that speaks the fake agent script (the stub in `tests/`). `npm run check && npm run lint && npm test`.
- **System:** on kind (`mise run dev:*`) and then oldie, a manual checklist:
  - open chat, ask for a read-only question, approve one edit, deny one command, see a hard-denied secret read;
  - reload the tab (resume);
  - trigger a pod-shape change (resume after restart);
  - two tabs (superseded).

## 8. Rollout and compatibility

1. Release order: **proto → sandbox-container → sandbox-proxy → ui → helm**. Each consumer bumps `enzarb-proto` via `cargo update -p enzarb-proto` / the npm version.
2. The sandbox image ships with chat enabled (`ENZARB_MAX_CHATS=1`) but inert: no client opens `acp` until the proxy and UI support it. A new image reaches each Project through the existing drain-first roll (the image is a pod-shape change, so `pendingChange` → drain → recreate). Until then that Project's chat page says "updates on the next sandbox restart". No forced rolls.
3. The proxy can deploy before or after the image; skew is handled by `features` (section 1).
4. UI behind `ENZARB_UI_CHAT` (helm `ui.chat.enabled`, default `false`). Turn it on for oldie after the system checklist passes, then make it the chart default in a later release. Deploys go through the updater as usual.
5. Rollback: turn the UI flag off, or set `ENZARB_MAX_CHATS=0` in the sandbox env. That is a pod-shape change, so it drains first. The flag is the fast path.
6. Docs: update `docs/contracts.md` §3 (filesystem: `worktrees/chat-*`, `.enzarb/chats/`), §4 (mux additions), §7 (proxy forwards features, idle timeout); `enzarb-proto/docs/protocol.md`; new **ADR-0014 "Interactive chat over ACP"** recording the decisions above (it extends ADR-0011).

## 9. Security review checklist

- [ ] No new credential or token type reaches the browser; the JWT path is unchanged.
- [ ] Proxy CTRL allowlist still rejects `run_*`. Only `open{kind:"acp"}` is newly allowed, and only the browser's `hello` is answered locally.
- [ ] sandbox-init forces `clientCapabilities` to none, `cwd` to the chat worktree and `mcpServers: []` on every `session/new`/`load`. A browser cannot point the agent at another cwd, MCP server or session id outside the registry.
- [ ] `session/set_mode` limited to `bypassPermissions` (only while `ENZARB_CHAT_ALLOW_BYPASS`), `default` and `plan` (amendment 2026-10-12; was `default`/`plan`); `acceptEdits`/`auto`/`dontAsk`, `authenticate` and unknown methods refused.
- [ ] Bypass default (amendment 2026-10-12): hard deny and permission prompts apply only in `default`/`plan`; the bounds are the scrubbed env, forced cwd/worktree, no MCP, gVisor, egress NetworkPolicy, redaction, non-dumpable sandbox-init and `.enzarb` 0700. Kill switches `ENZARB_CHAT_PERMISSION_MODE=default|plan`, `ENZARB_CHAT_ALLOW_BYPASS=false`, `ENZARB_MAX_CHATS=0`.
- [ ] Open residual risk: a bypass chat agent shares the sandbox-init uid and can write the shared clone's `.git/config`/`.git/hooks` used by authenticated `fetch`/`push`, and read the mux token, run callback files and job `/proc/<pid>/environ`; git-call hardening is a follow-up.
- [ ] Permission responses validated against the outstanding request: `*_always` and unknown ids become reject; there is no auto-approve path in UI or sandbox, and no timeout leads to allow.
- [ ] In `default`/`plan` mode, hard deny covers the Claude token file, `/var/run/enzarb/auth/token`, `.enzarb/`, secret paths, writes outside the chat worktree, `git push`, and fetch hosts outside the allowlist.
- [ ] No push/clone/deps/npm tokens in the chat agent env.
- [ ] Claude token (and its encodings) redacted from everything sent to the browser and from the transcripts.
- [ ] Line, prompt and idle limits, the chat cap and the hard lifetime are enforced in the sandbox, not the UI.
- [ ] Process group is killed on close, idle, supersede and shutdown; no orphan agents (reaper test).
- [ ] Chat transcripts are 0600 under `.enzarb/chats/` and pruned with the worktree.
- [ ] UI renders agent markdown/text safely: no `{@html}` of agent output without sanitising, and links get `rel="noopener noreferrer"`.
- [ ] Old proxy/sandbox never receives an `acp` open (feature gate), so no 1008 surprise on the terminal path.
- [ ] Cookie/Origin migration tracked as a follow-up, not silently dropped.

---

## Work breakdown (upstream first; one branch name `chat-acp` across repos)

**P1. enzarb-proto: `acp` channel kind, `welcome.features`, `open_error.code`**
- Scope: Rust `ctrl.rs` (`ChannelKind::{Acp, Unknown}`, `Open.repo?/resume?`, `Welcome.features`, `OpenError.code`), `session::open_acp`; TS `ctrl.ts`, `session.ts` (`features`, `openAcp` returning byte streams), `channel.ts`; `docs/protocol.md`; vectors.
- Acceptance: vectors pass in both languages; old messages still decode; a 2 MiB round-trip over `openAcp` under flow control; `cargo test`, `npm test`.
- Risk: low (additive). Size: M. Deps: none.

**P2. root docs: ADR-0014 and contracts update**
- Scope: ADR-0014; contracts §3/§4/§7; link this design.
- Acceptance: reviewed and merged.
- Risk: low. Size: S. Deps: can land alongside P1.

**S1. enzarb-sandbox-container: `Policy::hard_deny`**
- Scope: `policy.rs` hard-deny subset plus policy hint output; table tests.
- Acceptance: tests cover secrets, auth token, outside-worktree edits, `git push`, fetch hosts.
- Risk: medium (policy correctness). Size: S–M. Deps: none.

**S2. enzarb-sandbox-container: chat manager and ACP relay**
- Scope:
  - `src/chat.rs` (spawn per channel, JSON-RPC filter, permission tracking, redaction, transcript, idle/hard limits, cap with supersede, SIGTERM close);
  - `server.rs` dispatch of `kind:"acp"`, `welcome.features`;
  - `config.rs` `ENZARB_MAX_CHATS`, `ENZARB_CHAT_IDLE_SECS`, `ENZARB_CHAT_MAX_SECS`, `ENZARB_CHAT_RETENTION_DAYS`;
  - chat worktree create/sweep under `repo_lock`;
  - `CLAUDE_CONFIG_DIR` on the PVC.
- Acceptance: unit tests plus the fake-agent e2e (section 7) green; `docker build .`.
- Risk: high (security boundary, process lifecycle). Size: L. Deps: P1, S1.

**X1. enzarb-sandbox-proxy: allow `acp` opens, forward features, idle/ping**
- Scope: `guard.rs` allowlist; `relay.rs` local welcome includes `features ∩ {"acp"}`; `idle_timeout` default ≥ 35 min; WebSocket ping if missing.
- Acceptance: tests from section 7; `run_*` still rejected.
- Risk: medium. Size: S. Deps: P1.

**U1. enzarb-ui: project routes under a shared layout**
- Scope: already planned in the issue (PR 1).
- Risk: medium (shared state). Size: M. Deps: none (can start now).

**U2. enzarb-ui: ACP client, reducer, components**
- Scope: PR 2 of the plan plus feature/open_error states and the policy hint. Bump `@enzarb/proto`; add `@agentclientprotocol/sdk` (measure bundle size).
- Acceptance: section 6 tests.
- Risk: medium. Size: L. Deps: P1.

**U3. enzarb-ui: Chat page wiring and flag**
- Scope: `Chat.svelte`, `chat/+page.svelte`, the connection remote function, `ENZARB_UI_CHAT`, nav item, backoff, sessionStorage resume, Playwright smoke test against the stub.
- Risk: medium. Size: M. Deps: U1, U2.

**H1. enzarb-helm: values and image bumps**
- Scope: `ui.chat.enabled` (default false), proxy idle timeout value, sandbox `ENZARB_MAX_CHATS` passthrough (if the operator's env injection needs it; otherwise image default), chart version bump.
- Acceptance: `helm lint enzarb`.
- Risk: low. Size: S. Deps: S2, X1, U3 releases.

**O1. Enable on oldie and validate**
- Scope: run the system checklist (section 7); then flip the chart default.
- Size: S. Deps: H1.

**F1 (follow-up). enzarb-sandbox-proxy: cookie + Origin auth on `/sandbox/`**
- Scope: mirror enzarb-realtime#8 / enzarb-ui#71.
- Deps: those land.

Explicitly no changes: enzarb-workflows (drain ignores chat by design), enzarb-sandbox-operator (no spec field), enzarb-connectors.

## Open questions for the maintainer

- **Q1. Shared Claude quota.** Chat and jobs draw on the same subscription and rate limits. Is it acceptable that heavy chat use can slow or fail queued jobs? The alternative is pausing chat while `maxRuns` slots are full, which I do not recommend.
- **Q2. Private packages in chat.** The chat agent has no `npm_token`/`deps_token`, so `npm ci` of private `@enzarb/*` packages and same-Project git deps fail in a chat worktree. Should the executor's long-lived npm token be made available to the sandbox for chat? That would be a new standing secret in the sandbox. Recommendation: no for v1.
- **Q3. Pushing from chat.** v1 never pushes. Do you want a later "push branch / open PR from this chat" action? It would go through workflows with a fresh `clone_token`, not through the sandbox.

Everything else above is decided, with the reasoning given.
