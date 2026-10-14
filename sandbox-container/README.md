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
- `git`, `curl`, `ca-certificates`, `openssh-client`, `procps`
- Docker: `dockerd`, `containerd`, `runc`, the `docker` CLI, buildx and compose (see [Docker and privileges](#docker-and-privileges-adr-0016))

There is no rustup/Rust, npm, ripgrep, jq or other CLI tool, and no `HEALTHCHECK` (Kubernetes probes `/healthz`). `scripts/smoke.sh` asserts both what is present and what is absent.

## Docker and privileges (ADR-0016)

The image contains `dockerd`, `containerd`, `runc` and the `docker` CLI (static bundle 29.8.2, sha256-pinned) plus the buildx and compose plugins. `sandbox-init` runs `dockerd` and everything else:

- **Root pid 1, uid 1000 children.** The operator starts the `sandbox` container as root (the image has no `USER`; `scripts/smoke.sh` runs it that way). sandbox-init drops every process it spawns other than dockerd (agent runs, chat agents, git, `mise`, PTY shells) to uid/gid 1000 with `setgroups([1000])` and `PR_SET_NO_NEW_PRIVS` (`src/privs.rs`), with `HOME`, `USER` and `LOGNAME` set. PTYs go through `sandbox-init --exec-as-sandbox-user <cmd>`. Files it creates under `/workspace` are chowned to 1000. Without root (tests, `docker run --user 1000`) all of this is a no-op, and dockerd is expected to fail.
- **Supervision** (`src/dockerd.rs`). dockerd listens on `unix:///var/run/docker.sock` (`DOCKER_HOST`, group 1000, mode 0660), data root `/var/lib/docker` (the `docker-data` PVC), exec root `/var/run/dockerd`. Flags come from `ENZARB_DOCKERD_ARGS` (default: the gVisor set `--iptables=false --ip6tables=false --storage-driver=overlay2 --feature=containerd-snapshotter=false`). When the effective storage driver is not `vfs`, the leftover `<data-root>/vfs` of the old default is deleted before each dockerd start and the bytes freed are logged (images from the old driver are gone and get rebuilt or pulled). If overlay2 does not work on a volume, `/healthz` reports `dockerd.state=failed` with the reason (dockerd's own stderr is in the container log); go back with `runtimeProfiles.<class>.dockerdArgs` set to the flags above with `--storage-driver=vfs`. It is restarted with backoff (1 s to 30 s, reset after 60 s stable). `GET /healthz` is always 200; its JSON body carries `dockerd.state` (`starting`, `running`, `failed` with `reason`, `stopped`, `disabled`). If dockerd is missing or cannot start, agents and chats still work. On SIGTERM it is stopped after the chats and before the other children. Env: `ENZARB_DOCKERD=off`, `ENZARB_DOCKERD_BIN`, `ENZARB_DOCKERD_ARGS`, `ENZARB_DOCKERD_SOCKET`, `ENZARB_DOCKERD_DATA_ROOT`, `ENZARB_DOCKERD_EXEC_ROOT`.
- **Networking of inner containers.** dockerd runs with `--iptables=false` (gVisor), so the default bridge has no NAT and no egress. `/usr/local/bin/docker` is a wrapper (`scripts/docker-wrapper.sh`; real client in `/usr/local/libexec/docker/docker`) that adds `--network=host` to `run`, `create` and `build` unless you pass `--network`. "Host" is the sandbox container's network namespace; the Project's egress NetworkPolicy (DNS and 443) still applies. `-p` publishes nothing; compose services need `network_mode: host`. `ENZARB_DOCKER_NETWORK=default` disables the wrapper's default.
- **Known limitations.** Files written by inner containers to a bind mount are root-owned unless you pass `--user 1000:1000` (the policy allows it but cannot add it). dockerd, containerd and inner containers share the container's CPU and memory limits. Switching storage drivers on an existing data root hides the old images and containers. The real-dockerd smoke test is `ENZARB_SMOKE_DOCKERD=1 cargo test --test dockerd_smoke -- --ignored` (inside a gVisor sandbox).
- **Agent policy.** `docker version|info|build|buildx build|run|compose ...` are allowed. Denied: host namespaces, `--privileged`, caps, devices, bind mounts and build paths outside the worktree (which includes the docker socket and `/var/lib/docker`), `login`/`push`.

`scripts/smoke.sh` starts the image with the extra capabilities dockerd needs outside gVisor. If dockerd comes up it checks `docker version` as uid 1000 (and `docker run hello-world`, best effort); if the CI runner cannot host dockerd it asserts that `/healthz` reports the failure and skips the server checks.

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

- **Browser to agent.** `initialize` gets no client capabilities (no fs, no terminal). `session/new` and `session/load` get the chat worktree as `cwd` and no MCP servers. `session/load` works only for the session id the chat was resumed with, and only if the agent advertises `loadSession`. `session/prompt` accepts `text` and `resource_link` blocks, up to 256 KiB. `session/set_mode` accepts `bypassPermissions` (only while `ENZARB_CHAT_ALLOW_BYPASS` is true), `default` and `plan`; `acceptEdits`, `auto` and `dontAsk` are refused. `session/cancel` passes. Every request is rebuilt from known fields, so browser `_meta` never reaches the agent. Any other method gets `-32601`.
- **Agent to browser.** Only responses, `session/update` and `session/request_permission` are relayed, and the Claude token (and its base64 forms, and anything shaped like `sk-ant-…`) is redacted from them.
- **Permission mode.** Every session starts in `ENZARB_CHAT_PERMISSION_MODE`, `bypassPermissions` by default: the sandbox, not a per-call prompt, is the boundary for chat (agent jobs keep their own strict policy). `session/new` and `session/load` carry a fixed `_meta.claudeCode.options` = `{allowDangerouslySkipPermissions: <ENZARB_CHAT_ALLOW_BYPASS>, permissionMode: <mode>, settingSources: ["user"]}`. `claude-agent-acp` 0.88 ignores that `permissionMode`: it starts a session in the settings' `permissions.defaultMode` (user settings and the repo's `.claude/settings*.json`) or the mode of a resumed transcript. So when the `session/new`/`load` response does not report the configured mode in `modes.currentModeId`, sandbox-init sends its own `session/set_mode` and hands the response to the browser only after that succeeded, with `currentModeId` set to the mode (`modes.availableModes` passes unchanged). If the agent refuses the mode, the browser gets an error and no session. After the session is established and after every successful browser `session/set_mode`, the browser gets the `_enzarb/mode` notification `{sessionId, modeId, bypassAllowed}`. Mode changes made by the agent itself (leaving plan mode, which can only go to `default` because the elevated ExitPlanMode options are `allow_always`) arrive only as the agent's `session/update` `current_mode_update`.
- **Permissions.** In `bypassPermissions` Claude Code asks for nothing except its own bypass-immune checks, so the policy below only sees those: there is no hard deny for ordinary tool calls. In `default` and `plan`, `Policy::hard_deny` answers first: the agent gets `reject_once`, and the browser gets the `_enzarb/permission_denied` notification `{sessionId, toolCall, rule, reason}`. Everything else is forwarded with `_meta.enzarb = {policy, rule, reason}` as a hint. The browser may answer only an id it was sent, and only with an `allow_once` or `reject_once` option or `cancelled`. Anything else becomes `reject_once`. Close, idle and `session/cancel` answer every pending request `cancelled`. Nothing is ever approved automatically.
- **Environment.** The agent's environment is built from scratch: `PATH`, `HOME`, locale, the mise/cargo/rustup dirs, `DOCKER_HOST`, `SSL_CERT_*`, the git identity, `CLAUDE_CODE_OAUTH_TOKEN` and `CLAUDE_CONFIG_DIR=/workspace/.enzarb/chat-home/.claude` (on the PVC, so `session/load` works after a pod restart). There is no clone, deps or npm token.
- **Storage.** `/workspace/.enzarb/chats/<id>/meta.json` (the registry: ACP session id, clone, worktree) and `transcript.jsonl` (every relayed message, redacted), both 0600. The worktree is removed when the chat ends. The branch, and so any commit, stays in the clone, and resuming recreates the worktree from it. Chat worktrees left by a crash are removed at startup.

Chats do not use run slots and do not hold up drain.

What bounds a chat agent in bypass mode (for example after a prompt injection from repo content): its environment has no clone, push, deps or npm token; its cwd is its own worktree; it has no MCP servers and no client fs/terminal; the pod's egress NetworkPolicy and gVisor; the line, lifetime and idle limits. Every browser-bound message is still redacted. sandbox-init marks itself non-dumpable (`PR_SET_DUMPABLE`), which on Linux keeps a same-uid process out of its `/proc/<pid>/environ` and memory; enforcement under gVisor has not been verified.

What it can still read: the agent runs as the same uid (`enzarb`) as sandbox-init, run agents and their git/npm commands, so file modes do not separate them. `/workspace/.enzarb` is 0700 and the token and callback files are 0600, which only keeps other uids out. A bypass chat agent can read the Claude token (it has it anyway), the mux auth token file (`ENZARB_AUTH_TOKEN_FILE`), `/workspace/.enzarb/runs/*/callback.json` (run callback URLs and bearer tokens), `.enzarb/chat-home` and other chats' transcripts, every clone and worktree, and the environment of a concurrently running job's processes through `/proc/<pid>/environ` (which holds that job's deps/npm token while it runs). It can also write what later code runs: every clone's `.git/config` and `.git/hooks` (by path; see below), the run worktrees and the persisted toolchains that job agents run with their deps/npm token, and `chat-home/.claude/settings.json` (hooks and allow rules of later chats).

Authenticated git (issue #52). Because any agent can write a clone's config and hooks, sandbox-init never runs a git command that carries the clone/push token inside a clone (`src/netgit.rs`). Git always reads a repository's own config and `-c` cannot take back what it adds: with git 2.53 (the image's), `url.<evil>.insteadOf`/`pushInsteadOf` in `.git/config` still rewrites an explicit URL on the command line, and `include.path`, `http.proxy` or `http.sslCAInfo` would reroute the request. So each fetch and push runs in a fresh, empty bare repo (`.enzarb/net/<op>-<run_id>-<128 random bits>`, deleted afterwards; the operation fails if its config changed between `git init` and the end of the call; the network-relevant keys `http.proxy`, `http.sslVerify`, `http.sslCert`/`sslKey`, cookies and redirects are also pinned in env config) that borrows the clone's objects through `objects/info/alternates`; a fetch then copies the refs into the clone with a local, token-free `git fetch`. The first clone goes into such a random directory and is then moved into `repos/`. Those calls also get an empty process environment (only `PATH`, locale, CA and proxy variables pass), `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`, `HOME=/nonexistent` (no `.netrc`), `core.hooksPath=/dev/null`, `core.fsmonitor=false`, `core.sshCommand=false`, empty `credential.helper` and `core.askPass`, `protocol.allow=never` with only the URL's scheme (`https`; `file` in tests) allowed, an explicit URL instead of `origin`, and `push --no-verify`. The `Authorization` header is passed only as `GIT_CONFIG_*` environment, never written to a file. sandbox-init's token-free git calls in clones and worktrees also run with `core.hooksPath=/dev/null` and `core.fsmonitor=false`.

Chats get their own repository, `worktrees/chat-<id>`: a `git init` that borrows the clone's objects (alternates) and copies its `origin/*` refs and tags, not a `git worktree` of the clone. So the chat's `.git` is its own, and the clone's config and hooks are not one `../` away from its cwd. When the chat closes (or at startup after a crash) its branch `enzarb/chat-<id>` is fetched back into the clone and the directory is removed; resume recreates it from that branch. Cost: a checkout per open chat, as before, plus the chat's own new objects; the clone's objects are shared, not copied. If the clone's objects were garbage-collected while a chat still referenced them through the alternate, that chat repo would break; sandbox-init's own git calls run with `gc.auto=0`. A chat repo whose branch cannot be fetched back (deleted branch, missing registry entry) is moved to `worktrees/.lost-chat-<id>-<ms>` instead of being deleted.

What this does not fix: the chat agent still has the same uid, so it can still write the clone by its path, replace toolchain binaries jobs run, and read `/proc/<pid>/environ` of a concurrent git call that carries the token (git processes are not non-dumpable), or race the scratch repo between its creation and use (a randomly named dir, a config check and pinned network keys narrow this, but `url.*.insteadOf` cannot be reset and an inotify watcher on `.enzarb/net` could still win). Running chat agents under a separate uid is the fix for those. Set `ENZARB_CHAT_PERMISSION_MODE=default` (or `ENZARB_CHAT_ALLOW_BYPASS=false`) where that matters.

| Variable | Default | Effect |
|---|---|---|
| `ENZARB_MAX_CHATS` | `1` | Live chats at once. A new open at the cap closes the oldest chat with `superseded`. `0` disables chat: no `acp` feature, and opens get `chat_disabled`. |
| `ENZARB_CHAT_IDLE_SECS` | `1800` | No message in either direction for this long closes the chat with `idle`. A pending permission does not keep it alive. |
| `ENZARB_CHAT_MAX_SECS` | `28800` (8 h) | Total lifetime of one chat channel, then `limit`. |
| `ENZARB_CHAT_PERMISSION_MODE` | `bypassPermissions` | Mode every chat session starts in: `bypassPermissions`, `default` or `plan`. Any other value fails startup. Defaults to `default` when `ENZARB_CHAT_ALLOW_BYPASS=false`. |
| `ENZARB_CHAT_ALLOW_BYPASS` | `true` | `false` makes `bypassPermissions` unavailable: the adapter is told not to offer it and `session/set_mode` to it is refused. Combined with `ENZARB_CHAT_PERMISSION_MODE=bypassPermissions` it fails startup. |
| `ENZARB_CHAT_RETENTION_DAYS` | `7` | Chat records (meta, transcript) untouched for this long are deleted at startup. |

Fixed limits: an NDJSON line over 4 MiB in either direction closes the chat with `limit`, and a socket gets one `acp` channel (`chat_limit`).

Claude token (per user, ADR-0004 amendment 2026-10-13): each run uses `run_start.claude_token` and each chat `open.claude_token`, kept in memory, passed only as `CLAUDE_CODE_OAUTH_TOKEN` to the agent and redacted from transcripts and summaries. When the field is absent (old proxy or workflows), the legacy `/workspace/.enzarb/claude-token` is read instead. Token-bearing frames go only to a proxy that offers the `claude_user_token` client feature in `hello`. For such a peer, a `claude setup-token` capture is sent as `credential_captured{token}` and never written to disk, and a valid legacy file is promoted once per session with `credential_captured{token, legacy: true}` and deleted only when the proxy acks with `credential_state{configured: true}` and the file still holds the promoted token; without an ack it stays as the fallback. Any other peer (older proxies relay sandbox frames to the browser verbatim) gets the old behaviour: the capture is written to the legacy file (0600) and `credential_captured` carries no token, and nothing is promoted. The client kind and features are self-declared in `hello`; the only authentication is the shared bearer token.

`open_error` codes: `chat_disabled`, `chat_limit`, `no_credential` (neither `open.claude_token` nor a legacy token file), `repo_not_cloned` (`open.repo` names a repo no job has cloned yet; without `repo` the first clone is used), `invalid_request` (malformed `repo`/`resume`) and `unsupported_kind`.

`CLOSE` reasons: `idle`, `superseded`, `shutdown` (SIGTERM), `limit`, and `exit:<n>` / `signal:<n>` when the agent dies.
