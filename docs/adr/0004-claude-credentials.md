# 0004. Claude credentials via `claude setup-token`

## Context
Agents run Claude Code with the user's own Claude subscription. Since February 2026, Anthropic's terms prohibit third-party OAuth flows against Claude subscriptions.

## Decision
- The user runs a guided `claude setup-token` in the sandbox terminal.
- sandbox-init (pid-1) captures the resulting `sk-ant-oat01-` token and stores it for that user's sandboxes.

## Consequences
- Webhook-triggered runs spend a specific user's subscription. Choosing whose, and how the token is stored and protected, is open ([gaps.md](../gaps.md) #4) (see amendment below).

## Amendment 2026-10-13: one Claude token per user

Maintainer-approved in enzarb/enzarb#47. This amends the "Decision" above (storage per Project) and resolves the "whose token" part of [gaps.md](../gaps.md) #4. The capture method (guided `claude setup-token`, no third-party OAuth) is unchanged.

**Decision.**
- **One token per user.** It is stored as Secret `enzarb-claude-<owner>` in the shared sandbox namespace (ADR-0013), labelled `enzarb.dev/claude-credential=<owner>`, key `token`. `<owner>` is `spec.owner` of the user's Projects; the operator derives a DNS-safe name and hashes the login when it cannot be used directly. Encrypted Postgres was rejected: it needs a new key, key rotation and a decrypt path.
- **The proxy is the only writer.** sandbox-init has no Kubernetes API access (ADR-0013) and keeps none. On capture it sends `credential_captured{kind, token}` over the mux connection; `enzarb-sandbox-proxy` consumes the frame (never forwarded to the browser), upserts the Secret for the authenticated Project owner and answers the browser with `credential_state{configured:true, scope:"user"}`. The Secret name is always derived from the owner, never from frame content, and the label is checked before any update or delete. The proxy's Role is limited to `enzarb-sandboxes` and grants on `secrets` the verbs `get` (already namespace-wide per ADR-0013), `create`, `update`, `patch` and `delete`. `resourceNames` cannot pin any of them: the names are per owner and created at runtime, and `create` cannot be restricted by name. The compensating controls are the owner-derived name and the label check. See the ADR-0013 amendment for who else can read these Secrets. A browser `credential_clear` makes the proxy delete the owner's Secret.
- **The operator delivers it.** Each sandbox pod gets an `optional: true` read-only Secret volume for its owner's Secret at `/run/enzarb/claude/` (no `subPath`, so updates propagate) and `ENZARB_CLAUDE_TOKEN_FILE=/run/enzarb/claude/token`. The operator mirrors presence (never the value) into `Project.status`. `spec.owner` is immutable.
- **Jobs use the Project owner's token**, because a job runs in the owner's sandbox. An owner without a token gets a distinct, non-retried failure (`no_credential`, "owner has no Claude credential"); there is no fallback to another user's token.
- **Rotation without draining.** sandbox-init reads the token file at the start of each run and chat open; in-flight runs keep the value they started with. A run starting inside the kubelet propagation window (about a minute) may still see the old value.
- **Migration.** sandbox-init reads the user file first and falls back to the legacy `/workspace/.enzarb/claude-token`. On connect, if the legacy file exists and the user has no token, it emits `credential_captured` from it (the first Project wins; nothing is overwritten). Once the user file exists it deletes the legacy file. Re-running `setup-token` in a chosen Project replaces the Secret.

**Rollout order.** proto, operator, proxy, then the sandbox image. An old sandbox on a new proxy still works (the proxy consumes its `credential_captured`).

**Unchanged.** The token never reaches the browser, the UI or logs; exposure to a prompt-injected agent is as before. GitHub, npm and deps tokens are unaffected.
