# 0004. Claude credentials via `claude setup-token`

## Context
Agents run Claude Code with the user's own Claude subscription. Since February 2026, Anthropic's terms prohibit third-party OAuth flows against Claude subscriptions.

## Decision
- The user runs a guided `claude setup-token` in the sandbox terminal.
- sandbox-init (pid-1) captures the resulting `sk-ant-oat01-` token and stores it for that user's sandboxes.

## Consequences
- Webhook-triggered runs spend a specific user's subscription. Choosing whose, and how the token is stored and protected, is open ([gaps.md](../gaps.md) #4) (see amendment below).

## Amendment 2026-10-13: one Claude token per user, in Postgres

Maintainer-approved in enzarb/enzarb#47 (storage in Postgres, not Kubernetes Secrets: maintainer decision on #48). This amends the "Decision" above (storage per Project) and resolves the "whose token" part of [gaps.md](../gaps.md) #4. The capture method (guided `claude setup-token`, no third-party OAuth) is unchanged. [ADR-0013](0013-shared-sandbox-namespace.md) is unchanged: no new Secrets in `enzarb-sandboxes` and no new Secret RBAC for the proxy.

**Decision.**
- **Storage: one row per user** in `app.claude_credentials`, keyed by the GitHub login (`spec.owner` of the user's Projects). The token is encrypted at rest with AEAD (AES-256-GCM; `owner` and `key_id` are bound as associated data). The key comes from a Kubernetes Secret (`enzarb-claude-key`, namespace `enzarb`, a keyring with one active key) mounted only into the services that decrypt: sandbox-proxy and the workflows executor. The key id is stored with each row. Rotation: add the new key, roll the two services, run the re-encrypt job, then retire the old key. Details, functions and roles: [database.md](../database.md).
- **Who can read the ciphertext (ADR-0009 roles):** `enzarb_sandbox_proxy` (writer; also reads for chat opens) and `enzarb_workflows` (reader), both through `SECURITY DEFINER` functions in `app_api`. The UI's runtime role `enzarb_ui` cannot: it sees only the view `app_api.claude_credential_v` (`owner`, `present`, `updated_at`) and has no privilege on the table. Connectors and the operator have no access.
- **Capture, proxy is the only writer.** sandbox-init has no database access and keeps none. On capture it sends `credential_captured{kind, token}` over the mux connection and does not write the token to disk. `enzarb-sandbox-proxy` consumes the frame (never forwarded to the browser, never logged), encrypts it and calls `claude_credential_put` for the authenticated Project owner (`sub` equals `spec.owner`; the row key is never taken from frame content). It answers the browser with `credential_state{configured:true, scope:"user"}`. A browser `credential_clear` makes the proxy call `claude_credential_delete` for the owner.
- **Jobs get the owner's token per run.** The workflows executor reads and decrypts the Project owner's token when it builds each `RunStart` and sends it as `claude_token`, like `npm_token`: never stored in `jobs.input`, never written to disk by sandbox-init (environment of the agent process only), redacted everywhere. Rotation therefore needs no drain: the next run reads the new value and in-flight runs keep theirs. An owner without a token fails the job before dispatch with a distinct, non-retried failure (`no_credential`, "owner has no Claude credential"); there is no fallback to another user's token.
- **Chats get it when a chat opens.** The browser's `open{kind:"acp"}` passes through the proxy. The proxy drops any `claude_token` the browser put in it, reads and decrypts the authenticated owner's token and forwards the `open` to the sandbox with `claude_token` set (additive field on `open`; protocol version stays 1). With no token, the proxy answers the browser with `open_error{code:"no_credential"}` itself and does not forward. sandbox-init holds the token in the chat child's environment only. Rotation takes effect at the next chat open.
- **Migration.** Legacy per-Project token files (`/workspace/.enzarb/claude-token`) are promoted through the same capture path. On connect, if the legacy file exists, sandbox-init sends `credential_captured{kind, token, legacy:true}`. The proxy calls `claude_credential_put(..., only_if_absent=true)`, so the first Project wins and nothing is overwritten, and replies to the sandbox with `credential_state{configured:true, scope:"user"}`. On that reply sandbox-init deletes the legacy file, whether or not this Project's token won. A sandbox still using the legacy file keeps working until the migration happens. Re-running `setup-token` in a chosen Project replaces the user's token.
- **UI.** Project pages read the user-level presence from `app_api.claude_credential_v`. The operator has no part in this design: no Secret volume and no `Project.status` mirror.

**Rollout order.** proto (`claude_token` on `run_start` and `open`, `legacy` and `scope`, `credential_clear`), database migration and pgop Role, proxy, workflows, then the sandbox image, then the UI. Until the new sandbox image is rolled out, runs and chats use the sandbox's legacy file when `claude_token` is absent. Old decoders ignore the new fields.

**Unchanged.** The token never reaches the browser, the UI or logs; exposure to a prompt-injected agent while it runs is as before. GitHub, npm and deps tokens are unaffected.
