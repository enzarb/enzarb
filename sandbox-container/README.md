# enzarb-sandbox-container

The sandbox image (`ghcr.io/enzarb/enzarb-sandbox`) and its pid-1 `sandbox-init`.

```sh
docker buildx build --secret id=proto_key,src=$HOME/.ssh/id_ed25519 --load -t enzarb-sandbox:dev .
scripts/smoke.sh enzarb-sandbox:dev
```

## What is in the image

Only system components:

- `sandbox-init` (entrypoint)
- node 24, `@agentclientprotocol/claude-agent-acp` (pinned) and the glibc `claude` binary, installed directly (not through mise). `claude-agent-acp` and `claude` are symlinks in `/usr/local/bin`.
- `mise`
- `build-essential`, `pkg-config`, `libssl-dev`
- `git`, `curl`, `ca-certificates`, `openssh-client`

There is no rustup/Rust, npm, ripgrep, jq or other CLI tool, and no `HEALTHCHECK` (Kubernetes probes `/healthz`). `scripts/smoke.sh` asserts both what is present and what is absent.

## Toolchains come from mise

All user and project toolchains are installed by mise onto the `/workspace` PVC, so they persist per sandbox:

| Variable | Value |
|---|---|
| `MISE_DATA_DIR` | `/workspace/.toolchains/mise` |
| `MISE_CACHE_DIR` | `/workspace/.toolchains/cache` |
| `CARGO_HOME` / `RUSTUP_HOME` | `/workspace/.toolchains/cargo` / `.../rustup` |
| `MISE_YES=1` | never prompt (there is no tty) |
| `MISE_TRUSTED_CONFIG_PATHS=/workspace` | repo `mise.toml` files under `/workspace` are trusted (without it mise refuses to read them) |
| `MISE_IDIOMATIC_VERSION_FILE_ENABLE_TOOLS=rust` | read `rust-toolchain.toml` (idiomatic version files are off by default) |
| `PATH` | `/usr/local/bin`, then `/workspace/.toolchains/mise/shims`, then the system dirs |

`sandbox-init` passes its environment on to the run agent and to PTY shells, so agents see the shims and run project tools (`cargo`, `node`, ...) once those are installed. Chat agents get a scrubbed environment instead (see [Chat](#chat)).

### What a project repo needs

- A `mise.toml` with its tools, e.g. `[tools]` `rust = "1.99.0"`. A `rust-toolchain.toml` (channel, components, targets) alone is also picked up. Run `mise install` in the repo.
- node tools via mise get their own node; the system `node` only runs the agent adapter.

### Rust

mise installs Rust through rustup, whose installer needs `curl` (in the image), so `mise install` in a repo with a `mise.toml` or `rust-toolchain.toml` works directly.

Afterwards `cargo`, `rustc`, `clippy` and `rustfmt` resolve through the shims.

## Permission policy

The default command allowlist in `src/policy.rs` names programs, not paths. Entries for tools that are not baked in (`cargo`, `npm`, `rg`, `jq`, `tree`) are harmless: they match only once mise has installed the tool (or never). `rg`, `jq` and `tree` can be added per project with mise.

`python3` is allowed like `bash`/`sh`: only `python3 [-u -B -E -s -S -I -O -q] <script in the worktree> [args]`; `-c`, `-m`, stdin and other options are refused. Python is not baked in: pin it in the repo's `mise.toml` (`python = "3.13"`), otherwise the shim reports "command not found".

## Chat

The Chat page of the UI talks to Claude in the sandbox over an `acp` mux channel (ADR-0014, `docs/design/chat-tab.md` in the root repo). `welcome.features` contains `acp` only when chat is enabled and `claude-agent-acp` is installed.

For each `acp` channel, sandbox-init starts its own `claude-agent-acp` in `/workspace/worktrees/chat-<id>`, on branch `enzarb/chat-<id>`. It then filters the NDJSON JSON-RPC in both directions (`src/chat/filter.rs`):

- **Browser to agent.** `initialize` gets no client capabilities (no fs, no terminal). `session/new` and `session/load` get the chat worktree as `cwd` and no MCP servers. `session/load` works only for the session id the chat was resumed with, and only if the agent advertises `loadSession`. `session/prompt` accepts `text` and `resource_link` blocks, up to 256 KiB. `session/set_mode` accepts only `default` and `plan`. `session/cancel` passes. Every request is rebuilt from known fields, so browser `_meta` never reaches the agent. Any other method gets `-32601`.
- **Agent to browser.** Only responses, `session/update` and `session/request_permission` are relayed, and the Claude token (and its base64 forms, and anything shaped like `sk-ant-…`) is redacted from them.
- **Permissions.** `Policy::hard_deny` answers first: the agent gets `reject_once`, and the browser gets the `_enzarb/permission_denied` notification `{sessionId, toolCall, rule, reason}`. Everything else is forwarded with `_meta.enzarb = {policy, rule, reason}` as a hint. The browser may answer only an id it was sent, and only with an `allow_once` or `reject_once` option or `cancelled`. Anything else becomes `reject_once`. Close, idle and `session/cancel` answer every pending request `cancelled`. Nothing is ever approved automatically.
- **Environment.** The agent's environment is built from scratch: `PATH`, `HOME`, locale, the mise/cargo/rustup dirs, `DOCKER_HOST`, `SSL_CERT_*`, the git identity, `CLAUDE_CODE_OAUTH_TOKEN` and `CLAUDE_CONFIG_DIR=/workspace/.enzarb/chat-home/.claude` (on the PVC, so `session/load` works after a pod restart). There is no clone, deps or npm token.
- **Storage.** `/workspace/.enzarb/chats/<id>/meta.json` (the registry: ACP session id, clone, worktree) and `transcript.jsonl` (every relayed message, redacted), both 0600. The worktree is removed when the chat ends. The branch, and so any commit, stays in the clone, and resuming recreates the worktree from it. Chat worktrees left by a crash are removed at startup.

Chats do not use run slots and do not hold up drain.

| Variable | Default | Effect |
|---|---|---|
| `ENZARB_MAX_CHATS` | `1` | Live chats at once. A new open at the cap closes the oldest chat with `superseded`. `0` disables chat: no `acp` feature, and opens get `chat_disabled`. |
| `ENZARB_CHAT_IDLE_SECS` | `1800` | No message in either direction for this long closes the chat with `idle`. A pending permission does not keep it alive. |
| `ENZARB_CHAT_MAX_SECS` | `28800` (8 h) | Total lifetime of one chat channel, then `limit`. |
| `ENZARB_CHAT_RETENTION_DAYS` | `7` | Chat records (meta, transcript) untouched for this long are deleted at startup. |

Fixed limits: an NDJSON line over 4 MiB in either direction closes the chat with `limit`, and a socket gets one `acp` channel (`chat_limit`).

`open_error` codes: `chat_disabled`, `chat_limit`, `no_credential` (no Claude token), `repo_not_cloned` (`open.repo` names a repo no job has cloned yet; without `repo` the first clone is used), `invalid_request` (malformed `repo`/`resume`) and `unsupported_kind`.

`CLOSE` reasons: `idle`, `superseded`, `shutdown` (SIGTERM), `limit`, and `exit:<n>` / `signal:<n>` when the agent dies.
