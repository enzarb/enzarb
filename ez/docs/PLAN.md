# ez (`ez`) — plan

A Rust agentic coding agent in the spirit of Claude Code, with:

- **Lifecycle modes** instead of permission modes (explore → design → implement → test → review → release).
- **mise tasks as tools**, injected into context per mode (lazily when there are many).
- **MCP client**: servers from config / `.mcp.json` / ACP, exposed per mode as `mcp__<server>__<tool>`.
- **Hooks** (e.g. post-edit format/lint with automatic fix turns).
- **ACP-native**: the agent only speaks the Agent Client Protocol; the `ez` CLI/TUI is an ACP client.
- **AskUserQuestion** tool, surfaced through ACP.

## Workspace layout

```
crates/
  ez-core     agent loop, sessions, context assembly, modes, permissions, config
  ez-llm      Provider trait + Anthropic + OpenAI-compatible clients (streaming, tool use)
  ez-tools    built-in tools: Read, Write, Edit, Glob, Grep, Bash, AskUserQuestion, TodoWrite, SwitchMode
  ez-mise     mise task discovery, schema mapping, execution
  ez-mcp      MCP client (rmcp): connections, tool wrapping, lazy meta-tools
  ez-hooks    hook config, matching, runner, fix-turn feedback
  ez-acp      ACP JSON-RPC (stdio) agent server + client plumbing
  ez-cli      `ez` binary: ratatui TUI + headless `-p`, `ez acp`, `ez init`
```

ACP-first: the agent core is exposed only through ACP. `ez` launches the agent in-process over
a duplex pipe (or as an `ez acp` subprocess for editors such as Zed) and acts as an ACP client.

## Providers

`Provider` trait with a single internal message/tool representation:

- Anthropic Messages API (streaming, tool use, prompt caching).
- OpenAI-compatible Chat Completions (OpenAI, Ollama, OpenRouter, vLLM, …).

Configured as profiles in `~/.config/ez/config.toml`; each mode may set `profile` (which profile to use) and/or `model` (a model id) to override the session's. A mode naming an unconfigured profile falls back to the session profile with a start-of-session notice. Profiles list their available `models` (`/model`; `_ez/set_model` pins one per session, recorded as `model_change`).

### Onboarding and keys

`ez_llm::list_models` queries `/v1/models` (Anthropic, paginated) or `/models` (OpenAI-compatible). `ez_core::setup` plans and applies setup with `toml_edit` (comments preserved): user profile, `credentials.toml`, project `modes.toml` per-mode `profile`/`model`. Keys live only in `credentials.toml` (0600) as `[keys.<name>]`; profiles reference them with `api_key_name` (no env-var keys; a legacy hand-written `api_key_env` is still read). The wizard (`ez setup`, `/setup`, automatic on first interactive run) is a pure state machine in `ez-cli/src/tui/setup`; `ez keys` manages named keys; `EZ_CONFIG_DIR` relocates user config.

## Modes

No hidden built-ins at runtime. Modes are defined in `.ez/modes.toml` (project) and
`~/.config/ez/modes.toml` (user). With no configuration a single implicit `default` mode
exposes all tools and the UI hints to run `ez init`.

`ez init` writes `.ez/modes.toml` from templates embedded in the binary
(explore, design, implement, test, review, release; `--minimal` gives plan/build/verify;
`--modes a,b` selects a subset). It fills `mise_tasks` from the repo's real `mise tasks ls`
by name heuristics, leaving unmatched tasks in a commented list. `ez init --diff` compares the
file with current templates.

```toml
[modes.implement]
description = "Write code for the approved plan"
profile = "anthropic"          # optional: profile for this mode (default: session profile)
model = "claude-sonnet-5-5"    # optional: model id for this mode (beats --model and the profile)
prompt = "You are implementing the approved plan in plans/. Keep changes minimal."
tools = ["Read","Write","Edit","Glob","Grep","Bash","AskUserQuestion","SwitchMode","TodoWrite"]
mise_tasks = ["build", "gen:*"]
next = ["test", "review"]
auto_advance_to = []
permissions = { Bash = "ask", Edit = "allow", Write = "allow" }

[[modes.implement.hooks.post_edit]]
glob = "**/*.rs"
run = "mise run fmt"
on_failure = "fix_turn"   # ignore | block | fix_turn
max_fix_turns = 2
```

Mode transitions: the agent calls `SwitchMode`, which is surfaced as an ACP permission request
unless the target is in `auto_advance_to`. Users can switch via ACP `session/set_mode` (TUI hotkey).

## mise tasks as tools

1. Discover with `mise tasks ls --json`.
2. Map each task to a tool (`mise__<task>`), with an `args: string[]` parameter.
3. Only tasks matching the current mode's selectors are exposed.
4. When more than a threshold match, expose `MiseListTasks` / `MiseRun` meta-tools instead,
   so schemas are pulled on demand.
5. Run via `mise run <task> -- <args>`, streaming output as tool-call updates; output truncated.

## MCP servers

1. Config: `[mcp_servers.<name>]` in user/project `config.toml` plus project `.mcp.json`
   (Claude Code format); merged by name, project `config.toml` wins. `[mcp] lazy_threshold = 20`.
2. Connect on the first prompt (rmcp, stdio + streamable HTTP, 30s timeout each); failures become
   notices and a `mcp_servers` transcript record, never errors.
3. Tools are named `mcp__<server>__<tool>`; modes select them with `mcp_tools` globs (none by
   default; the implicit default mode gets `mcp__*`). `readOnlyHint` maps to kind `read` (allowed
   by default); everything else asks.
4. Above the threshold, `McpListTools` / `McpCall` replace the individual tools.
5. ACP `mcpServers` on `session/new` / `session/load` are added per session. `ez mcp list` inspects.

## Hooks

Events: `session_start`, `user_prompt_submit`, `pre_tool_use`, `post_tool_use`, `post_edit`,
`mode_change`, `stop`. Hooks receive JSON on stdin. `post_edit` hooks run once per turn over
the de-duplicated set of edited files (`{files}` placeholder). On failure: `ignore`, `block`
(report to user) or `fix_turn` (feed the output back to the model as a new turn, up to
`max_fix_turns`). Hooks can be global (`[[hooks.post_edit]]`) or mode-scoped.

## AskUserQuestion

Claude Code-compatible schema (questions, options, multiSelect, implicit "Other"). Sent to the
client as the `_ez/ask_user_question` ACP extension request when the client advertises
`_meta.ez.askUserQuestion`; otherwise falls back to `session/request_permission` per question
(single-select), and finally to returning an error telling the model to ask in plain text.

## Session history

Sessions and transcripts are stored **in the project** at `.ez/sessions/<YYYYMMDD-HHMMSS-xxxxxx>.jsonl`
so they are committed alongside the code they produced, for traceability. The transcript is
append-only JSONL (each record has `ts` and `type`) and is complete enough to audit and to resume:

- full user prompts, fix-turn prompts and tool-result messages;
- full assistant messages including thinking/reasoning blocks (with signatures);
- every tool call: input, output, error flag, permission decision, edits as unified diffs, duration;
- hook runs (command, exit code, output, duration), mode changes (who initiated, why), compactions;
- per-request metrics: model, usage, duration, time-to-first-token, tokens/sec; per-turn totals.

`ez --resume <id>` / ACP `session/load` rebuild history from the transcript and replay it to the client;
tool calls left dangling by a crash get synthetic "interrupted" results.

## Milestones

1. Workspace skeleton, both providers, core loop with Read/Write/Edit/Bash/Glob/Grep, headless CLI.
2. ACP server; CLI rewired as an ACP client; ratatui TUI.
3. Modes, permission system, `ez init` templates.
4. mise task discovery, injection, lazy loading.
5. Hooks including post-edit fix turns.
6. AskUserQuestion end-to-end, session persistence/load, compaction.
7. Tests (mock provider, ACP round trips) and CI.
8. MCP client (`ez-mcp`), per-mode selection, ACP passthrough, `ez mcp list`.
9. Onboarding: model discovery, setup wizard, named API keys, `/model`.
