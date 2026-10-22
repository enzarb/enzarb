# ez

`ez` is enzarb's coding agent ([ADR-0028](../docs/adr/0028-ez-coding-agent.md)), written in Rust. Instead of
permission modes it has **modes** (a system prompt, a tool set, mise tasks, MCP tools, hooks and a
model each), it exposes your **mise tasks as tools**, runs **hooks** (for example format/lint after
every edit, with automatic fix turns), and keeps a complete **transcript** of every session.

The agent speaks only the [Agent Client Protocol](docs/ACP.md) (ACP). The `ez` terminal UI and the
headless `ez -p` mode are ACP clients talking to an in-process agent, and `ez acp` serves the same
agent to sandbox-init, the enzarb Chat tab and editors such as Zed. See [docs/PLAN.md](docs/PLAN.md)
for the original design. It started as [ruckcode](https://github.com/ruckc/ruckcode).

Build and install from this directory: `cargo install --path crates/ez-cli` (installs `ez`).
Releases are tagged `ez/vX.Y.Z`; the sandbox image builds `ez` from source.

## Quick start

Configuration lives in `~/.config/ez/config.toml` (user) and `.ez/config.toml`
(project, overrides user per key). API keys are never read from your shell environment: the
setup wizard stores them in your per-user credentials file (see [Setup](#setup) and
[Named API keys](#named-api-keys)). Just run `ez` and follow the wizard:

```sh
cd your-project
ez                       # interactive TUI (first run starts the setup wizard)
ez "explain this repo"   # TUI with an initial prompt
```

Any OpenAI-compatible server works as a profile, for example Ollama:

```toml
# ~/.config/ez/config.toml
default_profile = "ollama"

[profiles.ollama]
provider = "openai"                  # "anthropic" | "openai" | "claude-code"
model = "qwen2.5-coder:14b"
base_url = "http://localhost:11434/v1"
# api_key_name = "ollama"           # a key from credentials.toml (omit for local servers)
# max_tokens = 32000                 # default 32000
# context_window = 200000            # default 200000
# thinking = "high"                  # passed through: Anthropic `thinking` object / OpenAI reasoning_effort
```

A `claude-code` profile keeps ez's prompts, modes, tools and hooks but runs the model loop in
Claude Code (ADR-0028). ez starts `claude-agent-acp` as a child (`EZ_CLAUDE_AGENT_ACP` overrides
the command) and authenticates through Claude Code itself (`CLAUDE_CODE_OAUTH_TOKEN` or its
login). The session is created with ez's system prompt in override mode, every Claude Code
built-in tool disabled, no Claude Code settings sources, and ez's tools served as the `ez` MCP
server on a loopback URL with a per-session bearer token. Every tool call goes through ez's
permissions and hooks.

```toml
[profiles.claude]
provider = "claude-code"
model = "sonnet"                     # passed to Claude Code; any id or alias it accepts
```

Current limits: the system prompt is fixed when the first prompt starts Claude Code, so a later
mode switch changes the tools but not the prompt. Claude Code compacts its own context. Resuming
a session (`session/load`) doesn't restore the Claude Code conversation yet.

Other top-level keys: `compaction_threshold` (default 0.8), `max_turn_requests` (default 100),
`[mise] lazy_threshold` (default 12), `[permissions]` (tool name or glob such as `"mise__*"` to
`allow` / `ask` / `deny`), and global `[[hooks.<event>]]` tables. `EZ_PROFILE` overrides
`default_profile`; `--profile` and `--model` override per run.

## Setup

The first time you run interactive `ez` without a usable profile (one with a stored key, or a
keyless local endpoint) and without a project `.ez/config.toml`, a setup wizard starts
automatically. Environment variables never count. Run it again any time with `ez setup` or the
`/setup` slash command. The steps, in order:

1. **API key**: paste it (masked), pick a saved named key, or choose "no key" (for local servers).
   Nothing is written until the final save; the secret is held only in memory until then.
2. **Endpoint URL** (required, `http(s)://...`, trailing slashes are trimmed). It is prefilled from
   the key's shape (`sk-ant-` Anthropic, `sk-or-` OpenRouter, `sk-` OpenAI) and offers quick fills:
   Anthropic `https://api.anthropic.com`, OpenAI `https://api.openai.com/v1`, OpenRouter
   `https://openrouter.ai/api/v1`, Ollama `http://localhost:11434/v1`, LM Studio
   `http://localhost:1234/v1`, vLLM `http://localhost:8000/v1` (Tab in the terminal, chips in the app).
3. **Check compatibility**: the wizard probes the URL with the key and detects the API: Anthropic
   Messages (`GET {url}/v1/models`) or OpenAI-compatible (`GET {url}/models`), trying the likelier one
   first. You see `Anthropic API ✓ (N models)` / `OpenAI-compatible API ✓ (N models)` or a specific
   failure (invalid key / HTTP 401, wrong path / HTTP 404 with a hint to add or remove `/v1`, connection
   refused, DNS, TLS). You can retry, force the other API flavor, or continue and type model ids by hand.
4. **Models** (Space toggles, `a` selects all, typing filters, `/` focuses the filter), then the default.
5. **Profile name** (default from the detected provider and host: `anthropic`, `openai`, `openrouter`,
   `ollama`, `lmstudio`, `vllm`, or the hostname), **models per mode**, and a **review** of exactly what
   will be written.

`Esc` goes back a step, `Ctrl+C` aborts without writing anything. `-p` and `ez acp` never start the
wizard; with no usable key they fail with an error that points at `ez setup`.

What gets written, and where:

- `~/.config/ez/config.toml`: the `[profiles.<name>]` entry (provider, default `model`,
  `models` available to `/model`, `base_url`, `api_key_name`) and `default_profile`.
  Existing comments and formatting are preserved. Set `EZ_CONFIG_DIR` to use another directory.
- `~/.config/ez/credentials.toml` (mode 0600, your per-user config directory): the pasted API
  key, saved as a named key called after the profile (`anthropic`, `openrouter`, ...). Keys are never
  written to `config.toml`, the project or the repository.
- `.ez/modes.toml` (the project, shared with your team): `profile` and `model` for each mode
  you assigned a model to. "Profile default" removes both keys. If the project has no `modes.toml`
  the wizard offers to run `ez init` first.

Scripted setup:

```sh
echo "$KEY" | ez setup --non-interactive --provider anthropic --api-key-stdin \
  --models claude-opus-5-5,claude-haiku-5-5 --mode-model explore=claude-haiku-5-5
ez setup --non-interactive --provider openai --base-url http://localhost:11434/v1 \
  --models qwen2.5-coder:14b          # local endpoint: no key needed
echo "$KEY" | ez setup --non-interactive --provider openai --base-url https://openrouter.ai/api/v1 \
  --api-key-name work --api-key-stdin --models anthropic/claude-sonnet-5 --profile openrouter
```

Flags: `--provider anthropic|openai` (`openai` = any OpenAI-compatible server) with `--base-url`
(required for `openai`, validated as an absolute http(s) URL; `anthropic` defaults to
`https://api.anthropic.com`), `--profile` (default: from provider and host, e.g. `ollama`),
`--models a,b` (required), `--default-model`, `--api-key-stdin` (reads the key from stdin and stores
it as a named key, called `--api-key-name` or else the profile name) or `--api-key-name NAME` alone
(an existing key). A key is required unless the endpoint is local/LAN (localhost, private ranges,
`*.local`). `--project-key` (see below), `--no-default`, and repeated `--mode-model MODE=MODEL`
(`MODE=default` clears; the model belongs to the profile being set up). The old `--api-key-env` flag
was removed.

Machine-readable modes (stable shapes, camelCase JSON on one stdout line;
errors go to stderr as `ez: message` with exit code 1; secrets are only ever read from stdin):

```sh
ez setup --json-state          # {needsOnboarding, project, defaultProfile, profiles[], keys{keys[],missing[]},
                               #  modes{status:"ready"|"missing"|"broken", ...}}
ez setup --non-interactive ... --json            # apply; prints {configPath, projectConfigPath,
                                                 #  credentialsPath, modesPath, modesUpdated}
ez setup --non-interactive ... --preview [--json]  # print the diff (or {diff}) and write nothing
ez setup --plan-stdin [--preview] --json         # the whole plan as JSON on stdin (incl. newKey.secret)
ez models --provider anthropic|openai [--base-url U] [--api-key-name N | --api-key-stdin] [--json]
                                                 # {models:[{id, displayName}]}
ez models --probe --base-url U [--provider anthropic|openai] [--api-key-name N | --api-key-stdin] --json
                                                 # detect the API: {kind|null, models[], attempts[{kind,url,status,error}], message}
ez keys list --json            # {keys:[{name, description, createdAt, masked, usedBy[]}], missing[]}
ez keys add NAME --api-key-stdin [--description D] --json   # {name, path}
ez keys remove NAME --json     # {name, stillUsedBy[]}
ez keys use NAME [--profile P] [--project] --json           # {name, profile, scope, path}
ez init --json [--minimal] [--modes a,b] [--force]          # {path, tasksDiscovered, modes{...}}
ez modes get --json                                         # the mode editor's document and catalog (below)
ez modes validate --doc-stdin --json                        # [{path, message, severity}]
ez modes set --doc-stdin [--preview] --json                 # {path, changed}, or {diff, changed, rewritten}
```

`ez modes` is the machine-readable mode/hook editor interface. The document is JSON
(camelCase) with one entry per mode covering every `modes.toml` field:

```jsonc
{ "defaultMode": "explore",
  "modes": [{ "id": "explore", "originalId": "explore",   // originalId: id in the file; a different id renames
              "name": "Explore", "description": "...", "prompt": "...",
              "tools": ["Read", "Grep"],                 // null = all built-in tools
              "miseTasks": ["docs*"], "mcpTools": ["mcp__github__*"],
              "next": ["design"], "autoAdvanceTo": [], "profile": null, "model": null,
              "permissions": { "Bash": "ask" },           // tool or glob -> allow | ask | deny
              "hooks": { "post_edit": [{ "run": "mise run fmt {files}", "glob": "*.rs", "matcher": null,
                                          "onFailure": "fix_turn", "maxFixTurns": 2, "timeoutSecs": 120 }] } }] }
```

`get` returns `{status: "missing"|"broken"|"ready", path, error?, defaultMode?, modes, globalHooks?,
catalog}`; `globalHooks` is the merged `[hooks]` of `config.toml`, shown read-only. The catalog holds
`builtinTools[{name, kind, readOnly}]`, discovered `miseTasks[{name, description}]`, `mcpServers`
(MCP tool names are only known while a server runs, so the editor offers `mcp__<server>__*` and
free-text globs), `profiles[{name, models}]`, `hookEvents`, `permissionValues` and the lifecycle
`templates`. `set` validates first (errors are refused with `ez: modes.toml would be invalid: ...`) and
edits the file with `toml_edit`: comments, ordering and formatting of everything you did not change
stay, removed keys go back to their defaults, and modes can be added, removed, renamed (headers are
rewritten, `originalId` ties the old and new id) and reordered. A mode whose layout cannot be edited in
place (inline tables, dotted keys) is written out fresh only if you change it, and listed in
`rewritten`.

`/model` lists the profile's `models` and switches the model for the current session (stored in the
transcript as a `model_change` record; `/model default` clears it).

### Named API keys

A profile finds its key through `api_key_name`, a name in your per-user `credentials.toml` (a missing
name is an error telling you to run `ez keys add NAME`). Environment variables are not used (an
old hand-written `api_key_env` still works for backward compatibility but is deprecated and never written).
Names are references, not secrets, so they are safe to commit; profiles can share a key, and a
project can pick its own:

```toml
# ~/.config/ez/credentials.toml (0600), managed by `ez keys` / the wizard
[keys.personal]
api_key = "sk-ant-..."
description = "my key"

# .ez/config.toml (committed): only this field is overridden, the rest of the user's
# [profiles.anthropic] (model, max_tokens, ...) still applies.
[profiles.anthropic]
api_key_name = "client-acme"
```

Profiles from user and project config are merged field by field, so a project config can override
just `api_key_name` (or `model`, ...) of a user profile. The wizard's first step offers a pasted key
(stored as a named key), an existing named key, or no key; `ez keys use NAME --project` points a
project at another key.

```sh
ez keys list                       # names, masked last 4 chars, descriptions, who uses them
ez keys add NAME [--description D] # secret from stdin or a hidden prompt
ez keys remove NAME
ez keys use NAME [--profile P] [--project]   # set api_key_name in user (or project) config
```

Team note: `modes.toml` references profile names. Teammates need profiles with the same names
(run `ez setup` or copy the `[profiles.*]` block). If a mode's `profile` is not configured,
`ez` warns at the start of the session ("mode `test` uses profile `openrouter`, which isn't
configured") and falls back to the default profile and its model instead of failing.

## Modes

Full reference for modes, every tool, MCP servers, permissions and hooks: [docs/modes.md](docs/modes.md).

Without configuration there is a single implicit `default` mode with all tools. Generate lifecycle
modes for a project:

```sh
ez init                         # writes .ez/modes.toml: explore, design, implement, test, review, release
ez init --minimal               # plan, build, verify
ez init --modes explore,implement,test   # a subset
ez init --diff                  # diff the existing file against current templates
ez init --force                 # overwrite an existing modes.toml
```

`ez init` fills each mode's `mise_tasks` from your repository's real `mise tasks ls`. Modes are
read from `.ez/modes.toml` and `~/.config/ez/modes.toml`. Switch modes in the TUI with
**Shift+Tab** (cycles) or `/mode <id>`, or start in one with `ez --mode <id>`. The agent can also
propose a switch with the `SwitchMode` tool; the user is asked unless the target is in
`auto_advance_to`.

Each mode can pick its own model or profile:

```toml
[modes.implement]
description = "Write code for the approved plan"
profile = "anthropic"          # optional: profile for this mode
model = "claude-sonnet-5-5"    # optional: model id (beats --model and the profile)
prompt = "Implement the approved plan. Keep changes minimal."
tools = ["Read", "Write", "Edit", "Glob", "Grep", "Bash", "AskUserQuestion", "SwitchMode", "TodoWrite"]
mise_tasks = ["build", "gen:*"]
next = ["test", "review"]
auto_advance_to = []
permissions = { Bash = "ask", Edit = "allow", Write = "allow" }
```

## mise tasks as tools

`ez` discovers tasks with `mise tasks ls --json` and exposes those matching the current mode's
`mise_tasks` selectors as tools named `mise__<task>` (argument: `args: string[]`), run via
`mise run <task> -- <args>` with truncated output streamed back as tool-call updates. When more than
`[mise] lazy_threshold` tasks match (default 12), the model gets two meta-tools, `MiseListTasks` and
`MiseRun`, instead, so task schemas are loaded only on demand.

## MCP servers

`ez` is an MCP client (built on the official [`rmcp`](https://crates.io/crates/rmcp) SDK; stdio and
streamable-HTTP transports, no OpenSSL). Servers come from, in increasing priority: user
`~/.config/ez/config.toml`, the project's `.mcp.json` (Claude Code format), and the project's
`.ez/config.toml`. Entries merge by server name; a later definition replaces the whole entry.

```toml
# config.toml
[mcp]
lazy_threshold = 20            # default

[mcp_servers.github]           # stdio server
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "..." }
# cwd = "tools"                # optional, relative to the project

[mcp_servers.docs]             # streamable HTTP server
url = "https://example.com/mcp"
headers = { Authorization = "Bearer ..." }
```

```json
// .mcp.json
{"mcpServers": {
  "github": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"], "env": {"GITHUB_TOKEN": "..."}},
  "docs": {"type": "http", "url": "https://example.com/mcp", "headers": {"Authorization": "Bearer ..."}}
}}
```

Servers are started on the first prompt of a session, concurrently with a 30s timeout each. A server
that fails to start is reported as a notice and recorded in the transcript (`mcp_servers` record), and
never blocks the others or the session.

Each tool is exposed as `mcp__<server>__<tool>` (sanitized to `[A-Za-z0-9_-]`, at most 64 chars).
**Modes opt in** with glob selectors over those full names; a mode without `mcp_tools` gets no MCP
tools (the implicit `default` mode gets `["mcp__*"]`):

```toml
[modes.implement]
mcp_tools = ["mcp__github__*", "mcp__docs__search"]
```

If a mode selects more than `[mcp] lazy_threshold` tools, the model gets `McpListTools` (optional
`server`) and `McpCall` (`tool`, `arguments`) instead of one tool per MCP tool.

**Permissions**: MCP tools ask by default, except tools whose `readOnlyHint` annotation is true, which
are allowed. Override with the usual globs in `[permissions]` (config or mode), e.g.
`"mcp__github__*" = "allow"` or `"mcp__github__delete*" = "deny"`. Results are text (non-text items
are described, e.g. `[image: image/png]`) and truncated to 30,000 bytes.

`ez mcp list` connects to the configured servers and prints each server's status and tool names.

Over ACP, `session/new` and `session/load` `mcpServers` (stdio and `http`) are added to that session
and override same-named configured servers; see [docs/ACP.md](docs/ACP.md). Child processes are killed
when the session ends.

## Hooks

Events: `session_start`, `user_prompt_submit`, `pre_tool_use`, `post_tool_use`, `post_edit`,
`mode_change`, `stop`. Hooks are shell commands (run via `sh -c` in the project root) that receive
JSON on stdin. They can be global (`[[hooks.post_edit]]` in config.toml) or per mode. `on_failure` is
`ignore` (default), `block` (report to the user) or `fix_turn` (feed the output back to the model
as a new turn, up to `max_fix_turns`, default 2). Other keys: `glob` (post_edit), `matcher`
(anchored regex on tool name for pre/post_tool_use), `timeout_secs` (default 120).
`post_edit` hooks run once per turn over the de-duplicated edited files (`{files}` placeholder).

```toml
[[modes.implement.hooks.post_edit]]
glob = "**/*.rs"
run = "mise run fmt"
on_failure = "fix_turn"
max_fix_turns = 2
```

## AskUserQuestion

The model can ask structured questions (options, multi-select, an implicit "Other") with the
`AskUserQuestion` tool. ACP clients that advertise `_meta.ez.askUserQuestion` get the
`_ez/ask_user_question` extension request; other clients get one `session/request_permission`
per question. See [docs/ACP.md](docs/ACP.md).

## Trying ez without an API key

A profile with `provider = "mock"` uses a deterministic demo model that reacts to the last
message, so you can exercise the TUI's question, permission, todo and streaming UI offline:

```toml
# .ez/config.toml
default_profile = "demo"
[profiles.demo]
provider = "mock"
model = "mock"
```

| Prompt | What the model does |
|---|---|
| `mock:ask` (or "ask me") | `AskUserQuestion`: one single-select, one multi-select |
| `mock:write <path> <text>` | `Write` (asks permission) |
| `mock:edit <path> <old> => <new>` | `Edit` |
| `mock:bash <cmd>` | `Bash` |
| `mock:todo` | `TodoWrite` with 3 items |
| `mock:mode <id>` | `SwitchMode` |
| `mock:think` | a thinking block plus text |
| `mock:tool <Name> <json>` | any tool call |
| anything else | lists these commands and echoes your prompt |

Tools are only called when the current mode offers them. Tool results are echoed back as text
("You chose: ...", "Tool `X` returned: ...", "Tool failed: ..."). Set `EZ_MOCK_DELAY_MS`
to slow the streamed chunks down. The `mock:` prefix avoids clashing with TUI `/` commands.

## Sessions and transcripts

Every session is written to `.ez/sessions/<YYYYMMDD-HHMMSS-xxxxxx>.jsonl` inside the project,
intended to be **committed with the code it produced** for traceability. The append-only JSONL
transcript holds full prompts, assistant messages including thinking blocks, every tool call (input,
output, permission decision, edits as diffs, duration), hook runs, mode changes, compactions, and
per-request metrics (model, usage, latency, time to first token, tokens/sec).

```sh
ez sessions              # list this project's sessions
ez sessions --json       # the same as one line of JSON (see below)
ez sessions show <id> --json [--offset N --limit N]   # one session's records, paged
ez sessions search <text> --json [--limit N]          # case-insensitive search over all sessions
ez -c                    # continue the most recent session
ez -r                    # pick a session interactively
ez -r 20250102-030405-a1b2c3   # resume by id (also --resume=<ID>)
ez -r "fix the bug"      # not an id: treated as the prompt, opens the picker
```

The JSON forms are machine-readable. Output is one
line of camelCase JSON; errors are a single `ez: message` line on stderr with exit code 1. Only
`<project>/.ez/sessions/` is ever read and ids are restricted to letters, digits, `-` and `_`.
Truncated or corrupt lines are skipped and counted in `skippedLines`.

- `ez sessions --json [--query TEXT]`: `[{id, createdAt, updatedAt, firstPrompt, mode, model, turns,
  toolCalls, inputTokens, outputTokens, durationMs, sizeBytes, recordCount, skippedLines}]`, most
  recently updated first (`--query` filters by id, first prompt, mode or model).
- `ez sessions show <id> --json`: `{meta, records, total, offset, limit, skippedLines}` where `meta`
  is the list row plus `cwd`, `profile`, `ezVersion`, and each record is `{index, ts, type, ...}`
  with camelCase fields (`assistant_message` carries `usage` and `metrics`, `tool_result` carries
  `permission`, `durationMs` and `edits` as unified diffs, and so on). `--limit` defaults to 500
  (maximum 5000).
- `ez sessions search <text> --json`: `[{id, recordIndex, type, snippet}]` over prompts, assistant text,
  reasoning, tool names, inputs and outputs, diffs, hook output and compaction summaries.

## Headless mode

```sh
ez -p "summarise the failing tests"
git diff | ez -p "review this diff"            # stdin is appended to the prompt
ez -p --allow Bash,Edit "fix the lint errors"  # auto-allow these tools
ez -p --yolo "do the thing"                    # allow every permission request
ez -p --output-format stream-json "..."        # raw session/update notifications, one per line
```

Other flags: `--mode`, `--profile`, `--model`, `--cwd`, `-c`, `-r <ID>`. Without `--allow`/`--yolo`
permission requests are denied; AskUserQuestion is cancelled. Exit codes: `0` finished (end of turn),
`130` cancelled (Ctrl+C), `1` any error or other stop reason.

## Editor integration (ACP)

`ez acp` serves the agent over newline-delimited JSON-RPC on stdio. In Zed:

```json
{
  "agent_servers": {
    "ez": {
      "command": "ez",
      "args": ["acp"]
    }
  }
}
```

Wire types come from the official [`agent-client-protocol-schema`](https://crates.io/crates/agent-client-protocol-schema)
crate (ACP v1, stable surface only) and the JSON-RPC transport from the official
[`agent-client-protocol`](https://crates.io/crates/agent-client-protocol) SDK. Supported methods (including `session/list`, `session/resume`,
`session/close` and the `model` config option), notifications and the `_ez/*` extensions are
documented in [docs/ACP.md](docs/ACP.md).

## Workspace layout

```
crates/
  ez-core     agent loop, sessions, context assembly, modes, permissions, config
  ez-llm      Provider trait + Anthropic and OpenAI-compatible clients
  ez-tools    Read, Write, Edit, Glob, Grep, Bash, AskUserQuestion, TodoWrite, SwitchMode
  ez-mise     mise task discovery, schema mapping, execution
  ez-mcp      MCP client (rmcp): server connections, tools as `mcp__<server>__<tool>`
  ez-hooks    hook config, matching, runner, fix-turn feedback
  ez-acp      ACP agent server + client plumbing (official ACP SDK for JSON-RPC)
  ez-cli      `ez` binary: TUI, headless `-p`, `ez acp`, `ez init`
  ez-workflow `.ez/workflows.toml`: schema, validation, templates, run stepping, dry runs (ADR-0029)
```

## Development

ez's crates are members of the enzarb root Cargo workspace. From the repo root:

```sh
cargo test -p ez-acp -p ez-claude -p ez-cli -p ez-core -p ez-hooks -p ez-llm -p ez-mcp -p ez-mise -p ez-tools -p ez-workflow
cargo clippy -p ez-acp -p ez-claude -p ez-cli -p ez-core -p ez-hooks -p ez-llm -p ez-mcp -p ez-mise -p ez-tools -p ez-workflow --all-targets -- -D warnings
cargo fmt --all
```

Set `EZ_LOG=debug` for logs (the TUI logs to `~/.cache/ez/ez.log`). Provider tests run
against a local fake SSE server and need no API keys.

## License

MIT — see [LICENSE](LICENSE).
