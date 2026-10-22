# 0011. Agent sessions through the Agent Client Protocol

## Context
The first implementation shelled out to `claude -p`. That gave a single JSON result at the end, no live progress, and a coarse permission switch (`acceptEdits` blocks every shell command; `bypassPermissions` allows everything).

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
- **Later use:** the same adapter backs interactive chat (ADR-0014).

## Amendment 2026-10-15: no permission policy
The repository owner decided to remove the permission policy (the soft allow-list `Policy::decide`, the hard deny-list `Policy::hard_deny` and `ENZARB_PERMISSION_POLICY`). sandbox-init now answers every `session/request_permission` of a session with the allow option (`allow_always` if offered, else `allow_once`) and still records each one in the transcript (`decision: "allow"`, `rule: "sandbox"`). Chats (ADR-0014) do the same, except that leaving plan mode is still the user's call. The security boundary is the sandbox itself: gVisor, the Project's egress NetworkPolicy, no Kubernetes API access, and tokens scoped to what the run needs (the push token never enters the agent's environment; the CI `GH_TOKEN` is read-only). `ENZARB_PERMISSION_POLICY` is accepted and ignored. Residual exposure is listed in `docs/gaps.md`.
