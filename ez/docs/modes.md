# Configuring ez: modes, tools, MCP servers, hooks and permissions

This is the reference for the files that control what ez does: `.ez/modes.toml` (modes) and `.ez/config.toml` (profiles, MCP servers, global permissions and hooks). The same configuration applies under every driver: `anthropic` and `openai` (ez's own loop) and `claude-code` (Claude Code's loop with ez's prompt and tools, [ADR-0028](../../docs/adr/0028-ez-coding-agent.md)).

- [Where configuration comes from](#where-configuration-comes-from)
- [Modes](#modes) and the [mode reference](#mode-reference)
- [The system prompt](#the-system-prompt)
- [Tools](#tools): [built-in tools](#built-in-tools), [SwitchMode](#switchmode), [mise tasks](#mise-tasks), [MCP tools](#mcp-tools)
- [Adding MCP servers](#adding-mcp-servers)
- [Permissions](#permissions)
- [Hooks](#hooks)
- [Profiles and models per mode](#profiles-and-models-per-mode)
- [Under the claude-code driver](#under-the-claude-code-driver)
- [In enzarb sandboxes](#in-enzarb-sandboxes)
- [A complete example](#a-complete-example)

## Where configuration comes from

| File | Holds | Precedence |
|---|---|---|
| `<project>/.ez/modes.toml` | modes | used if present; the whole file wins, nothing is merged |
| `~/.config/ez/modes.toml` | modes | used if the project has none |
| (built in) | one implicit `default` mode | used if neither file exists |
| `~/.config/ez/config.toml` | profiles, MCP servers, `[permissions]`, `[hooks]`, limits | lowest |
| `<project>/.mcp.json` | MCP servers (Claude Code format) | above the user config |
| `<project>/.ez/config.toml` | same keys as the user config | highest |
| `~/.config/ez/credentials.toml` | named API keys (`ez keys`) | never committed |

`EZ_CONFIG_DIR` replaces `~/.config/ez`. Config files merge key by key (tables merge, other values are replaced). MCP servers merge by server name, and a later definition replaces the whole server entry. Modes don't merge: a project `modes.toml` replaces the user's completely.

`ez init` writes a starter `.ez/modes.toml` (explore, design, implement, test, review, release; `--minimal` gives plan, build, verify). It fills each mode's `mise_tasks` from the repository's real tasks.

## Modes

A mode is one phase of work. It sets:
- a section of the system prompt (`description` and `prompt`);
- which tools the model sees (`tools`, `mise_tasks`, `mcp_tools`);
- what those tools may do without asking (`permissions`);
- shell hooks that run on agent events (`hooks`);
- which model runs it (`profile`, `model`);
- which modes it can hand over to (`next`, `auto_advance_to`).

A session is always in exactly one mode. It starts in `default_mode` (or `ez --mode <id>`, or the ACP `session/new` mode). The user switches with Shift+Tab or `/mode <id>`; the model proposes switches with the [`SwitchMode`](#switchmode) tool.

```toml
# .ez/modes.toml
default_mode = "plan"           # optional; defaults to the first mode in the file

[modes.plan]
description = "Read the code and write a plan"
prompt = "Do not edit files. End with a numbered plan and switch to build."
tools = ["Read", "Glob", "Grep", "TodoWrite", "AskUserQuestion"]
next = ["build"]

[modes.build]
description = "Implement the plan"
tools = ["Read", "Write", "Edit", "Glob", "Grep", "Bash", "TodoWrite"]
mise_tasks = ["build", "test*", "lint"]
mcp_tools = ["mcp__github__get_*"]
next = ["plan"]
permissions = { Write = "allow", Edit = "allow", Bash = "ask" }

[[modes.build.hooks.post_edit]]
glob = "**/*.rs"
run = "cargo fmt -- {files}"
on_failure = "fix_turn"
```

The file must define at least one mode. Mode ids are the `[modes.<id>]` keys, and the file order is kept (it orders Shift+Tab and the mode list in the prompt).

### Mode reference

| Key | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | id, capitalized | Display name (`[modes.impl] name = "Implementing"`). |
| `description` | string | `""` | One line. Shown in the mode list and at the top of the mode's prompt section. |
| `prompt` | string | `""` | Instructions added to the system prompt while the mode is active. |
| `tools` | string list | all built-ins | Allowlist of [built-in tools](#built-in-tools). Omit the key for all of them; `[]` for none. Affects built-ins only (see below). |
| `mise_tasks` | glob list | `[]` (none) | [mise tasks](#mise-tasks) to expose, matched against task names (`"test*"`, `"gen:*"`, `"*"`). |
| `mcp_tools` | glob list | `[]` (none) | [MCP tools](#mcp-tools) to expose, matched against full names (`"mcp__github__*"`). |
| `next` | mode id list | `[]` = every other mode | Modes `SwitchMode` may propose from here. |
| `auto_advance_to` | mode id list | `[]` | Of those, the switches that happen without asking the user. |
| `permissions` | table | `{}` | Tool name or glob to `"allow"`, `"ask"` or `"deny"`. Checked before `[permissions]` in config.toml. |
| `hooks` | table | `{}` | Per-mode [hooks](#hooks); they run after the global hooks for the same event. |
| `profile` | string | session profile | A profile from config.toml used while this mode is active. |
| `model` | string | profile's model | Model id used while this mode is active. |

Validation happens when a session starts: an undefined `default_mode` or a `next`/`auto_advance_to` entry naming an unknown mode is an error; an unknown name in `tools` is only a warning in the log, and the tool simply isn't there.

What `tools` does and doesn't control:
- It filters the eight built-in tools only. Listing `mise__build` or `mcp__github__search` in `tools` has no effect: mise tasks come from `mise_tasks` and MCP tools from `mcp_tools`.
- `SwitchMode` is added automatically whenever the mode has a switch target, whether or not it's listed.
- A tool the mode doesn't offer can't be called at all; the call fails with `unknown tool`. This is the strongest control a mode has: prefer leaving a tool out over denying it.

## The system prompt

ez builds the system prompt fresh for every model request (under the claude-code driver, once per session; see [below](#under-the-claude-code-driver)), in this order:

1. ez's base instructions (be concise, prefer dedicated tools over shell, prefer mise tasks, plan with TodoWrite, ask with AskUserQuestion, propose phase changes with SwitchMode, never commit or push unless asked).
2. `## Environment`: working directory, platform, date, git branch.
3. `## Project instructions (<file>)` for each of `AGENTS.md`, `CLAUDE.md` and `.ez/INSTRUCTIONS.md` that exists in the project root, in that order.
4. `## Current mode: <name>` with the mode's `description` and `prompt`.
5. `## Modes`: every mode's id and description (only when there is more than one mode).

So repository-wide guidance belongs in `AGENTS.md` / `CLAUDE.md` / `.ez/INSTRUCTIONS.md`, and phase-specific guidance belongs in the mode's `prompt`. Tool descriptions and schemas reach the model separately, with the tool list.

## Tools

Every tool the model can call is one of:

| Source | Names | Selected by |
|---|---|---|
| Built-in | `Read`, `Write`, `Edit`, `Glob`, `Grep`, `Bash`, `TodoWrite`, `AskUserQuestion` | mode `tools` |
| Mode switching | `SwitchMode` | automatic when there are switch targets |
| mise | `mise__<task>`, or `MiseListTasks` + `MiseRun` | mode `mise_tasks` |
| MCP | `mcp__<server>__<tool>`, or `McpListTools` + `McpCall` | mode `mcp_tools` |

### Built-in tools

Paths may be absolute or relative to the project root.

| Tool | Kind | Default permission | Input | Behaviour |
|---|---|---|---|---|
| `Read` | read | allow | `file_path`, `offset?` (1-based line), `limit?` | Text file with line numbers (`cat -n` style). 2000 lines by default; lines longer than 2000 characters are cut. |
| `Write` | edit | ask | `file_path`, `content` | Creates or overwrites a file (and parent directories). The diff is shown to the client and the file counts as edited for `post_edit` hooks. |
| `Edit` | edit | ask | `file_path`, `old_string`, `new_string`, `replace_all?` | Exact string replacement. `old_string` must be unique unless `replace_all` is true. Counts as an edit for hooks. |
| `Glob` | search | allow | `pattern`, `path?` | Files matching a glob (`**/*.rs`), respecting `.gitignore`, newest first, at most 500. |
| `Grep` | search | allow | `pattern` (regex), `path?`, `glob?`, `output_mode?` (`files_with_matches` default, `content`, `count`), `case_insensitive?` | Content search respecting `.gitignore`, at most 500 results. |
| `Bash` | execute | ask | `command`, `description?`, `timeout?` (ms) | `sh -c` in the project root. 2-minute default timeout, at most 10 minutes. Combined stdout/stderr and exit code, truncated in the middle to 30,000 characters. Cancelling the turn kills it. |
| `TodoWrite` | think | allow | `todos: [{content, status}]` (`pending`, `in_progress`, `completed`) | Replaces the session's plan; shown as a checklist in the client. |
| `AskUserQuestion` | other | allow | `questions`: 1-4 of `{question, header?, multiSelect?, options: 2-4 of {label, description?}}` | Structured questions to the user; free text is always possible. Needs an interactive client ([ACP.md](ACP.md)). |

### SwitchMode

Offered when the current mode has at least one target: the modes in `next`, or every other mode if `next` is empty. Input: `mode_id` (one of the targets) and `reason`. The user is asked to approve unless the target is in `auto_advance_to`. After a switch the new mode's tools apply from the next model request, and `mode_change` hooks run.

### mise tasks

ez runs `mise tasks ls --json` in the project when the session starts and keeps the tasks whose names match the mode's `mise_tasks` globs (hidden tasks are skipped). Each becomes a tool:
- Name: `mise__<task>`, with characters outside `[A-Za-z0-9_-]` replaced by `_` (`test:unit` → `mise__test_unit`), at most 64 characters.
- Description: the task's `description` plus its `usage` spec, if any.
- Input: `args: string[]`, passed as `mise run <task> -- <args>` (no shell, so no quoting issues).
- Kind execute (asks by default). 10-minute timeout; output streamed to the client and truncated to 30,000 characters.

When more tasks match than `[mise] lazy_threshold` (config.toml, default 12), the model gets two meta-tools instead: `MiseListTasks` (search kind, allowed) and `MiseRun` (`task`, `args`; execute kind, asks). This keeps large task sets out of the context until needed. Permission rules still name the real task (`mise__test`) or a glob (`"mise__*"`).

mise tasks are the preferred way to give a mode build, test and lint abilities: the task is reviewed in the repository, takes no free-form shell, and its name says what it does. A mode that needs only those tasks doesn't need `Bash` at all.

`EZ_MISE_BIN` overrides the `mise` executable.

### MCP tools

Tools from [MCP servers](#adding-mcp-servers) are named `mcp__<server>__<tool>` (sanitized to `[A-Za-z0-9_-]`, at most 64 characters). A mode sees only the ones matching its `mcp_tools` globs; a mode without `mcp_tools` sees none. (The implicit `default` mode, used when there is no modes file, selects `mcp__*`.)

```toml
[modes.triage]
mcp_tools = ["mcp__github__get_*", "mcp__github__search_*", "mcp__docs__*"]
```

When more tools match than `[mcp] lazy_threshold` (default 20), the model gets `McpListTools` (`server?`; lists names, descriptions and schemas) and `McpCall` (`tool`, `arguments`) instead. `McpCall` can only reach the tools the mode selected.

A tool whose MCP annotations say `readOnlyHint: true` counts as read kind (allowed by default); every other MCP tool asks by default. Results are converted to text (non-text items are described, e.g. `[image: image/png]`) and truncated to 30,000 characters.

## Adding MCP servers

Declare servers in config.toml (user or project) or in the project's `.mcp.json`; then select their tools per mode with `mcp_tools`. Declaring a server alone exposes nothing.

```toml
# .ez/config.toml
[mcp]
lazy_threshold = 20                     # default

# stdio: ez starts the process and talks over stdin/stdout
[mcp_servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "..." }
cwd = "tools"                           # optional, relative to the project

# streamable HTTP
[mcp_servers.docs]
url = "https://docs.example.com/mcp"
headers = { Authorization = "Bearer ..." }
```

| Key | Applies to | Meaning |
|---|---|---|
| `type` | both | `"stdio"` or `"http"` (`"streamable-http"` also accepted). Inferred from `command` / `url` when omitted. `"sse"` is not supported. |
| `command`, `args` | stdio | Program and arguments (not run through a shell). |
| `env` | stdio | Extra environment variables for the process. |
| `cwd` | stdio | Working directory, relative to the project. |
| `url` | http | Streamable HTTP endpoint. |
| `headers` | http | Request headers (e.g. `Authorization`). |

The same servers in Claude Code's format:

```json
// .mcp.json
{"mcpServers": {
  "github": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"],
             "env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "..."}},
  "docs": {"type": "http", "url": "https://docs.example.com/mcp",
           "headers": {"Authorization": "Bearer ..."}}
}}
```

Values are used literally: ez doesn't expand `${VAR}` in `env` or `headers`. Keep secrets in the user config (`~/.config/ez/config.toml`), not in committed project files.

Servers start on a session's first prompt, all at once, with a 30-second timeout each. A server that fails is reported as a notice and recorded in the transcript (`mcp_servers` record); the others and the session carry on. `ez mcp list` connects to every configured server and prints its status and tool names, which is the quickest way to find the names to put in `mcp_tools`.

ACP clients can also pass servers in `session/new` / `session/load` `mcpServers` (stdio and `http`); they apply to that session and replace configured servers of the same name ([ACP.md](ACP.md)).

## Permissions

Every tool call is checked before it runs. The first rule that matches decides:

1. The mode's `permissions`: an exact tool name, then globs.
2. config.toml `[permissions]`: an exact tool name, then globs.
3. The default: allow for read, search and think kinds (`Read`, `Glob`, `Grep`, `TodoWrite`, `MiseListTasks`, `McpListTools`, read-only MCP tools) and `AskUserQuestion`; ask for everything else.

```toml
# .ez/config.toml: project-wide rules
[permissions]
"mise__*" = "allow"
"mcp__github__*" = "ask"
"mcp__github__delete*" = "deny"

# .ez/modes.toml: this mode only
[modes.build.permissions]
Write = "allow"
Edit = "allow"
Bash = "ask"
```

- `allow` runs the tool. `deny` returns an error result to the model without running it. `ask` asks the client (allow once, allow always for the rest of the session, or reject). `ez -p` has no one to ask: it rejects `ask` calls unless the tool is named in `--allow Bash,Edit` or `--yolo` is given. For unattended use, give modes explicit `allow` rules or leave the tool out.
- Globs use `*`, `?` and `[...]`. When several globs in one table match, the first in alphabetical order wins, so keep overlapping globs in different tables (mode vs config) or use exact names.
- Leaving a tool out of the mode is stronger than `deny`: the model never sees it.

## Hooks

Hooks are shell commands (`sh -c`, in the project root) that run on agent events. Global hooks go in config.toml as `[[hooks.<event>]]`; per-mode hooks go in modes.toml as `[[modes.<id>.hooks.<event>]]`. For each event, global hooks run first, then the mode's, one after another.

| Event | When | JSON on stdin (always has `event`, `session_id`) | `matcher` / `glob` |
|---|---|---|---|
| `session_start` | first prompt of a session | | |
| `user_prompt_submit` | each user prompt, before the model sees it | `prompt` | |
| `pre_tool_use` | after the permission check, before the tool runs | `tool_name`, `tool_input` | `matcher` |
| `post_tool_use` | after a tool ran | `tool_name`, `tool_input`, `tool_response`, `is_error` | `matcher` |
| `post_edit` | when the model finishes a turn, if files were edited during it | `files` | `glob` |
| `mode_change` | after a mode switch | `from`, `to` | |
| `stop` | the turn is over (no fix turn due) | | |

| Key | Default | Meaning |
|---|---|---|
| `run` | (required) | Shell command. In `post_edit`, `{files}` is replaced by the matching files, shell-quoted. |
| `matcher` | match all | `pre_tool_use` / `post_tool_use`: regex on the tool name, anchored (`"Write\|Edit"`, `"mise__.*"`). |
| `glob` | match all | `post_edit`: only files matching it (tested on the relative path and the file name); the hook is skipped when none match. |
| `on_failure` | `ignore` | What a non-zero exit (or timeout) does; see below. |
| `max_fix_turns` | 2 | `fix_turn` only: how many fix turns in a row before giving up. |
| `timeout_secs` | 120 | Kill the hook after this long (counts as a failure). |

Environment: `EZ_EVENT`, `EZ_PROJECT_DIR`, `EZ_SESSION_ID`, and for `post_edit` `EZ_FILES` (newline-separated). Output (stdout and stderr) is kept up to 20,000 characters and recorded in the transcript.

What `on_failure` does per event:

| `on_failure` | `user_prompt_submit` | `pre_tool_use` | `post_tool_use` | `post_edit` | others |
|---|---|---|---|---|---|
| `ignore` | nothing | nothing | nothing | nothing | nothing |
| `block` | prompt dropped, user told | tool not run, model gets the hook output as an error | nothing | user told | user told |
| `fix_turn` | nothing | nothing | hook output appended to the tool result | output sent to the model as a new turn ("please fix"), up to `max_fix_turns` | nothing |

The usual pattern is a formatter or linter on `post_edit` with `fix_turn`, so the model fixes what it broke before the turn ends:

```toml
[[modes.build.hooks.post_edit]]
glob = "**/*.{ts,svelte}"
run = "npx prettier --write {files} && npx eslint {files}"
on_failure = "fix_turn"
max_fix_turns = 2
```

and a guard on `pre_tool_use` with `block`:

```toml
[[hooks.pre_tool_use]]
matcher = "Bash"
run = "! jq -r .tool_input.command | grep -Eq 'git push|rm -rf /'"
on_failure = "block"
```

## Profiles and models per mode

Profiles are defined in config.toml (`[profiles.<name>]`, see the [README](../README.md)). A mode can switch the profile, the model, or both while it's active:

```toml
[modes.plan]
profile = "claude"              # a claude-code profile for planning
model = "opus"

[modes.build]
profile = "local"               # an OpenAI-compatible local model for the edits
```

Model resolution, first match wins: a `/model` pin for the session, the mode's `model`, `--model` (only when the mode uses the session's own profile), the profile's `model`. A mode naming a profile that isn't configured falls back to the session profile, with a notice.

## Under the claude-code driver

With a `provider = "claude-code"` profile, Claude Code runs the model loop but everything on this page still comes from ez:
- The system prompt above replaces Claude Code's own.
- Claude Code's built-in tools are disabled. The mode's tools are served to it as the MCP server `ez`, so the model sees `mcp__ez__Read`, `mcp__ez__mise__build`, `mcp__ez__mcp__github__search`, …; built-in names the model emits anyway (`Read`) are routed to ez's tool of that name.
- Every call runs inside ez: the same permissions, hooks, transcript and edit tracking.
- Claude Code's own configuration is not loaded: no `CLAUDE.md` through Claude Code (ez adds it to the prompt itself), no Claude Code settings or hooks, and no `.mcp.json` through Claude Code (ez connects to those servers and re-serves the selected tools).

Limits today: the system prompt is fixed when the session's first prompt starts Claude Code, so a later mode switch changes the tools but not the prompt; Claude Code compacts its own context, so `compaction_threshold` doesn't apply.

## In enzarb sandboxes

> The behaviour in this section is decided ([ADR-0028](../../docs/adr/0028-ez-coding-agent.md)) but not built yet. What starts runs (events, workflows) is being redesigned ([ADR-0027](../../docs/adr/0027-workflow-clean-slate.md)).

When enzarb runs ez unattended in a sandbox:
- **Modes come from the default branch.** The run uses `.ez/modes.toml` (and `AGENTS.md`, `CLAUDE.md`, `.ez/INSTRUCTIONS.md`) from the default-branch head, pinned for the run and passed to ez. Edits to these files on the branch being worked on don't affect that branch's own runs.
- **No defaults.** A run without `.ez/modes.toml`, or naming a mode that doesn't exist, fails with that reason; there is no built-in fallback mode.
- **Each run names a mode.** The run's instruction becomes the first user message; the mode supplies the system prompt, the tools and the hooks.
- **The tool list is the permission boundary.** Unattended runs have no one to answer `ask`, and every call a mode offers is allowed. Design each mode as exactly the tools it needs; prefer `mise_tasks` over `Bash`.
- **No interactive tools.** `AskUserQuestion` and `SwitchMode` aren't offered in unattended runs; whatever starts the run picks the mode, not the model.

## A complete example

```toml
# .ez/config.toml (committed)
default_profile = "claude"

[profiles.claude]
provider = "claude-code"
model = "sonnet"

[mcp_servers.docs]
url = "https://docs.example.com/mcp"

[permissions]
"mise__*" = "allow"

[[hooks.pre_tool_use]]
matcher = "Bash"
run = "! jq -r .tool_input.command | grep -Eq 'git (push|reset --hard)'"
on_failure = "block"
```

```toml
# .ez/modes.toml (committed)
default_mode = "triage"

[modes.triage]
description = "Understand an issue and decide what to do"
prompt = """
Read the issue and the relevant code. Do not change files. Reply with: the root cause or the \
missing piece, the files involved, and a short plan."""
tools = ["Read", "Glob", "Grep"]
mcp_tools = ["mcp__docs__*"]
next = ["implement"]

[modes.implement]
description = "Make the change and prove it works"
prompt = "Keep the change minimal. Add or update tests. Run the checks before finishing."
tools = ["Read", "Write", "Edit", "Glob", "Grep", "TodoWrite"]
mise_tasks = ["build", "test*", "lint"]
next = ["review"]
permissions = { Write = "allow", Edit = "allow" }

[[modes.implement.hooks.post_edit]]
glob = "**/*.rs"
run = "cargo fmt -- {files}"
on_failure = "fix_turn"

[modes.review]
description = "Review the diff like a senior engineer"
prompt = "Look for bugs, missing tests and unclear code. Report findings; do not edit."
tools = ["Read", "Glob", "Grep", "Bash"]
mise_tasks = ["test*", "lint"]
permissions = { Bash = "deny" }
```

`review` lists `Bash` and then denies it only to show the difference: the model sees the tool but every call fails. Leaving `Bash` out of `tools` would be cleaner.
