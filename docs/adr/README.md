# Architecture decision records

Format: Context / Decision / Consequences. Status is `Accepted` unless noted. ADRs 0002–0005 restore decisions from the original design (2026-08-13); its plan file was lost, so these are rebuilt from notes. ADRs written before 0015 name the old per-component repos (`enzarb-ui`, `enzarb-helm`, …); those are now directories of the monorepo (`ui/`, `helm/`, …), and the text is left as written.

| # | Title | Date |
|---|---|---|
| [0001](0001-repo-split.md) | One repo per component, plus a top-level `enzarb` repo (superseded by 0015) | 2026-10-08 |
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
| [0015](0015-monorepo.md) | Monorepo | 2026-10-13 |
| [0016](0016-dockerd-in-the-sandbox-container.md) | dockerd in the sandbox container, supervised by sandbox-init (supersedes the sidecar of 0012) | 2026-10-14 |
| [0017](0017-work-item-dependencies.md) | Work item dependencies | 2026-10-14 |
| [0018](0018-epics.md) | Epics: breaking a big issue into sub-issues | 2026-10-14 |
| [0019](0019-sandbox-utilization-polling.md) | Polling sandbox utilization from the sandbox-init API | 2026-10-15 |
| [0020](0020-graph-workflows.md) | Graph workflows: definitions in the repo, a runner in the sandbox, a thin kernel outside | 2026-10-15 |
| [0021](0021-shared-workflow-library.md) | Shared workflow library: steps, prompts and graphs as data, nothing compiled in | 2026-10-16 |
| [0022](0022-workflow-engine-rewrite.md) | Workflow engine rewrite: a generic interpreter, every workflow in YAML (proposed; supersedes 0020, 0021) | 2026-10-16 |
| [0023](0023-manual-sandbox-roll.md) | Manual sandbox roll: a generation-bound annotation and a 30m drain timeout | 2026-10-17 |
| [0024](0024-liveness-findings.md) | Liveness findings: pure evaluation, a periodic `platform` source, observation only | 2026-10-18 |
| [0026](0026-platform-wait.md) | Platform wait: the engine polls platform facts, the step resumes on the same run, nothing times out | 2026-10-18 |
