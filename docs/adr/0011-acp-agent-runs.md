# 0011. Agent runs through the Agent Client Protocol

## Context
The first run implementation shelled out to `claude -p`. That gave a single JSON result at the end, no live progress, and a coarse permission switch (`acceptEdits` blocks every shell command; `bypassPermissions` allows everything).

## Decision
The user decided (2026-10-08) that sandbox-init drives Claude through [`@agentclientprotocol/claude-agent-acp`](https://github.com/agentclientprotocol/claude-agent-acp), the ACP adapter built on Anthropic's Claude Agent SDK. sandbox-init is the ACP client.
- **Session mode:** `default`.
- **Permissions:** each `session/request_permission` is decided by a declarative policy (`ENZARB_PERMISSION_POLICY`), and every decision is logged to the transcript.
  - The built-in policy allows reads and edits inside the worktree and an allowlist of build, test and git commands (git push excluded).
  - It denies everything else.
  - It never grants persistent permissions.

## Consequences
- **Gains:** structured live transcripts (tool calls, diffs, plans), per-tool permission control, and a clean `session/cancel`.
- **Image:** must ship node plus the pinned adapter, and run as non-root with a writable `HOME`.
- **Possible later use:** the same adapter can back interactive agent sessions in the UI.
