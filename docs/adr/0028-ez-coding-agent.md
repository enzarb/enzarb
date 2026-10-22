# 0028. `ez`: enzarb's own coding agent, one configuration for every model

Status: Proposed (2026-10-22). Amends [ADR-0004](0004-claude-credentials.md), [ADR-0011](0011-acp-agent-runs.md) and [ADR-0014](0014-chat-tab-acp-over-mux.md). Input to the workflow redesign of [ADR-0027](0027-workflow-clean-slate.md).

## Context
Agent runs (ADR-0011) and chats (ADR-0014) run Claude Code through `claude-agent-acp`. Claude Code brings its own system prompt, tool set and permission model, so what a step can do and how it is told to do it is decided partly by Claude Code, not by enzarb. Other models (local or OpenAI-compatible endpoints) would need another agent with different prompts, tools and permissions, and every backend's permission system would have to be configured separately.

[ruckcode](https://github.com/ruckc/ruckcode) was a test bed for these ideas: a Rust coding agent that speaks ACP, with lifecycle **modes** (system prompt, tool allow-list, mise tasks as tools, MCP tool selection, hooks, model per mode), Anthropic and OpenAI-compatible providers, and a full JSONL transcript.

## Decision

### 1. `ez` is the only agent runtime enzarb launches
ruckcode is copied (fresh, without history) into the top-level `ez/` component and renamed: binary `ez`, crates `ez-*`, project directory `.ez/`, environment `EZ_*`, ACP extensions `_ez/*`. The desktop app, WSL support, the download site and self-update are removed; the TUI, headless `ez -p`, `ez acp`, `ez setup` and `ez keys` stay. It releases as `ez/vX.Y.Z` and is built into the sandbox image from source.

The Chat tab and agent runs (the run and session primitives that ADR-0027 kept in the mux protocol and sandbox-init) both run `ez acp`, and so will whatever orchestration the ADR-0027 redesign introduces. ez owns everything except the model call: system prompt, tools (Read, Write, Edit, Glob, Grep, Bash, TodoWrite, `mise__*`, MCP tools, `submit_result`), hooks, the permission decision and the transcript. The same configuration produces the same prompts and the same tools on every backend.

### 2. Two drivers
| Driver | Agent loop | Model access | Credential |
|---|---|---|---|
| `claude-code` | Claude Code, through `claude-agent-acp` as a child of ez | Claude subscription | the user's `claude setup-token` token (`CLAUDE_CODE_OAUTH_TOKEN`), unchanged from ADR-0004 |
| `native` | ez's own loop | Anthropic Messages API, or any OpenAI-compatible endpoint (local models) | an API key / endpoint as named secrets |

The `claude-code` driver keeps Claude Code as the client that uses the subscription token, and configures it so that it behaves like ez:
- `session/new` `systemPrompt: {mode: "override"}` with ez's system prompt (not Claude Code's preset);
- `tools: []`: no Claude Code built-in tools;
- `mcpServers: [{type: "http", name: "ez", url, headers}]`: ez's tools for the active mode, served by the ez process itself on 127.0.0.1 with a per-session bearer token, so tool calls run in ez's code with the session's mode, hooks and transcript;
- `_meta.claudeCode.options`: `toolAliases` (`Read` → `mcp__ez__Read`, …) so built-in names the model emits reach ez's tools, `allowedTools: ["mcp__ez__*"]`, `settingSources: []` and `strictMcpConfig: true` (no `CLAUDE.md`, settings hooks or project `.mcp.json` from the worktree), and the mode's model.

ez never sends the subscription token to the Anthropic API itself.

### 3. One permission model
Permissions are decided only in ez, per mode. Under the `claude-code` driver Claude Code has no tools of its own and allows the `mcp__ez__*` tools, so it decides nothing. In sandboxes ez allows every call the mode offers (the ADR-0011 amendment): a mode's tool list is the control.

### 4. Modes come from the repository's `.ez/modes.toml`
The configuration format (modes, tools, MCP servers, permissions, hooks) is documented in [`ez/docs/modes.md`](../../ez/docs/modes.md).

- A mode is a system prompt section plus tools, mise task and MCP tool selectors, hooks, and an optional profile/model.
- Whatever starts an unattended agent run names a mode and gives an instruction. The instruction is the first user turn; the mode is the system prompt and tool set. How events map to runs, and so to (mode, instruction), is part of the ADR-0027 redesign.
- **Trusted copy only.** Unattended runs use `.ez/modes.toml` (and the instruction files ez folds into the system prompt) from the default-branch head, pinned per run and passed to ez, never the copy in the checkout being worked on. A branch under work cannot change its own modes.
- **No defaults.** An unattended run without a pinned `.ez/modes.toml`, or naming an unknown mode, fails with that reason instead of falling back to a built-in mode.

### 5. Local models
A Project points at an external OpenAI-compatible endpoint with a URL and key (Project named secrets); its egress NetworkPolicy must allow that host. Parity aims: one prompt for every backend (no per-backend prompt variants), small per-mode tool sets with lazy meta-tools, schema-validated `submit_result` with retries, per-profile context budgets, ez-side output truncation. A parity eval suite (recorded tasks replayed per backend) follows later.

## Consequences
- enzarb controls prompts, tools and permissions in one place, and switching a mode between Claude and a local model changes only the model.
- Claude Code stays in the image, but only as the agent loop of the `claude-code` driver.
- Claude Code features outside ez's tool set (sub-agents, web fetch, skills) are not available under ez unless ez adds them as tools.
- Claude Code truncates large MCP outputs and times out long MCP calls; ez truncates first and raises `MCP_TOOL_TIMEOUT` for the child.
- ADR-0014's plan-mode exception becomes an ez mode.
- One more component (`ez/`) to build, test and release.

## Rollout
1. Done: `ez/` import (rename, trim, edition 2024, CI).
2. Done (spike, tested against a fake child): the `claude-code` driver and the in-process MCP tool server (`ez/crates/ez-claude`); open items in [gaps.md](../gaps.md) #25.
3. ez for unattended use: inline pinned modes, keys from the environment in managed mode, sessions outside the worktree, `submit_result` as an ez tool, usage in the shapes sandbox-init reads.
4. sandbox-container: build ez into the image, run `ez acp` for agent runs and chats.
5. With the ADR-0027 redesign: a mode on run start (additive proto change), resolving and pinning `.ez/modes.toml` from the default branch, Project secrets for local-model endpoints, and enzarb's own `.ez/modes.toml`.
