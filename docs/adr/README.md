# Architecture decision records

Format: Context / Decision / Consequences. Status is `Accepted` unless noted. ADRs 0002–0005 restore decisions from the original design (2026-08-13); its plan file was lost, so these are rebuilt from notes. ADRs written before 0015 name the old per-component repos (`enzarb-ui`, `enzarb-helm`, …); those are now directories of the monorepo (`ui/`, `helm/`, …), and the text is left as written.

ADRs 0008, 0010, 0017–0022, 0024 and 0026 described the removed workflow engine and tasks and were deleted ([0027](0027-workflow-clean-slate.md)); their numbers are not reused.

| # | Title | Date |
|---|---|---|
| [0001](0001-repo-split.md) | One repo per component, plus a top-level `enzarb` repo (superseded by 0015) | 2026-10-08 |
| [0002](0002-sandbox-tenancy.md) | Upstream agent-sandbox, Capsule tenancy, Project CRD (tenancy superseded by 0013) | 2026-08-13 |
| [0003](0003-mux-protocol.md) | Single websocket with a custom mux protocol (enzarb-proto) | 2026-08-13 |
| [0004](0004-claude-credentials.md) | Claude credentials via `claude setup-token` | 2026-08-13 |
| [0005](0005-ui-sveltekit.md) | SvelteKit remote functions as the UI logic layer | 2026-08-13 |
| [0006](0006-scope-no-environments.md) | Scope: no Environment CR, no deploys | 2026-10-08 |
| [0007](0007-connectors.md) | Source-agnostic connectors (superseded by 0027) | 2026-10-08 |
| [0009](0009-database-sharing.md) | Shared database via pgop, one schema per owning component | 2026-10-08 |
| [0011](0011-acp-agent-runs.md) | Agent runs through the Agent Client Protocol | 2026-10-08 |
| [0012](0012-docker-in-sandbox.md) | Docker inside every sandbox, isolated by gVisor | 2026-10-08 |
| [0013](0013-shared-sandbox-namespace.md) | One shared sandbox namespace; no Kubernetes API access from sandboxes | 2026-10-10 |
| [0014](0014-chat-tab-acp-over-mux.md) | Interactive chat over ACP, carried on a new mux channel kind | 2026-10-12 |
| [0015](0015-monorepo.md) | Monorepo | 2026-10-13 |
| [0016](0016-dockerd-in-the-sandbox-container.md) | dockerd in the sandbox container, supervised by sandbox-init (supersedes the sidecar of 0012) | 2026-10-14 |
| [0023](0023-manual-sandbox-roll.md) | Manual sandbox roll: a generation-bound annotation and a 30m drain timeout | 2026-10-17 |
| [0025](0025-release-timing.md) | Release timing: updater ConfigMap and Project `status.lastRoll` | 2026-10-18 |
| [0027](0027-workflow-clean-slate.md) | Workflow clean slate: delete the engine, event hooks and tasks | 2026-10-21 |
