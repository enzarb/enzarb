# Architecture decision records

Format: Context / Decision / Consequences. Status is `Accepted` unless noted. ADRs 0002–0005 restore decisions from the original design (2026-08-13); its plan file was lost, so these are rebuilt from notes.

| # | Title | Date |
|---|---|---|
| [0001](0001-repo-split.md) | One repo per component, plus a top-level `enzarb` repo | 2026-10-08 |
| [0002](0002-sandbox-tenancy.md) | Upstream agent-sandbox, Capsule tenancy, Project CRD (tenancy superseded by 0013) | 2026-08-13 |
| [0003](0003-mux-protocol.md) | Single websocket with a custom mux protocol (enzarb-proto) | 2026-08-13 |
| [0004](0004-claude-credentials.md) | Claude credentials via `claude setup-token` | 2026-08-13 |
| [0005](0005-ui-sveltekit.md) | SvelteKit remote functions as the UI logic layer | 2026-08-13 |
| [0006](0006-scope-no-environments.md) | Scope: no Environment CR, no deploys | 2026-10-08 |
| [0007](0007-connectors.md) | Source-agnostic connectors | 2026-10-08 |
| [0008](0008-postgres-workflow-engine.md) | In-house workflow engine on Postgres | 2026-10-08 |
| [0009](0009-database-sharing.md) | Shared database via pgop, one schema per owning repo | 2026-10-08 |
| [0010](0010-approval-and-progress.md) | Plan approval and progress signals | 2026-10-08 |
| [0011](0011-acp-agent-runs.md) | Agent runs through the Agent Client Protocol | 2026-10-08 |
| [0012](0012-docker-in-sandbox.md) | Docker inside every sandbox, isolated by gVisor | 2026-10-08 |
| [0013](0013-shared-sandbox-namespace.md) | One shared sandbox namespace; no Kubernetes API access from sandboxes | 2026-10-10 |
| [0014](0014-chat-tab-acp-over-mux.md) | Interactive chat over ACP, carried on a new mux channel kind | 2026-10-12 |
