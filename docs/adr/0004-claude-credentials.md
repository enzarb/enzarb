# 0004. Claude credentials via `claude setup-token`

## Context
Agents run Claude Code with the user's own Claude subscription. Since February 2026, Anthropic's terms prohibit third-party OAuth flows against Claude subscriptions.

## Decision
- The user runs a guided `claude setup-token` in the sandbox terminal.
- sandbox-init (pid-1) captures the resulting `sk-ant-oat01-` token and stores it for that user's sandboxes.

## Consequences
- Webhook-triggered runs spend a specific user's subscription. Choosing whose, and how the token is stored and protected, is open ([gaps.md](../gaps.md) #4).
