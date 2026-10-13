# 0009. Shared database via pgop, one schema per owning repo

## Context
enzarb-ui, enzarb-connectors and enzarb-workflows all need shared state. Postgres is provisioned with pgop.

## Decision
- **One pgop `Cluster` (`enzarb-pg`) and one `Database` (`enzarb`).**
- **One schema per owning repo:** `workflows`, `connectors`, `app`. Only the owner migrates its schema.
- **Cross-repo access goes only through API schemas:** views and `SECURITY DEFINER` functions in `workflows_api` and `app_api`.
- **Two roles per service:** an owner role for migrations and a runtime role that can only read and write data. Credentials come from pgop's deterministic Secrets (Role secrets `enzarb-pg-<role>-credentials`).
- **Live UI updates** come from `NOTIFY work_item_changed`.
- **Projects stay as k8s CRs.**

See [database.md](../database.md).

## Consequences
- The API schemas are versioned contracts: additive changes, or new versioned objects.
- Splitting workflows onto its own Cluster later is possible without changing the contract, but it would lose the single-step insert-and-notify.
- pgop's current limits (one replica, no documented backups or pooling) must be addressed before production.

## Amendment 2026-10-13
- The proxy gets a runtime role `enzarb_sandbox_proxy` for the encrypted per-user Claude token (`app.claude_credentials`), reached only through `app_api` functions; the UI role sees presence only. See [database.md](../database.md) and [ADR-0004](0004-claude-credentials.md).
