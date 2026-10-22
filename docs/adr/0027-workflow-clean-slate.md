# 0027. Workflow clean slate: delete the engine, event hooks and tasks

Status: Accepted (2026-10-21). Supersedes ADR-0007 (connectors). The ADRs on the workflow engine, work item dependencies, epics, plan approval, graph workflows, the shared workflow library, liveness findings, platform wait and sandbox utilization polling were deleted with the code they described.

## Context
The workflow engine (a Postgres reducer, executors and ticker), its event hooks (a normalized event catalog, bindings, custom events, a YAML workflow library in a separate repo) and the tasks system (work items, board, dependencies, epics, approval signals) had grown too complicated to reason about or change. The owner wants to redesign them from scratch.

## Decision
- Delete the `workflows/` component, the `connector` and `connector-github` crates, the tasks UI and everything that only served them (the `engine`, `engine_api`, `workflows` and `workflows_api` schemas, `spec.workflows` on the Project CRD, the `workflows` Deployment and its RBAC, `.enzarb/workflows*` project files).
- `connectors/` keeps only the `receiver`. `POST /hooks/github` verifies the signature and logs each delivery in `connectors.events`, then runs `pg_notify('connector_event', id)`. Nothing else happens to an event: no routing, no workflow, no comments.
- The UI shows the logged events per project (Events tab); realtime pushes `event` frames filtered by a `repos` JWT claim.
- The redesign is a later decision with its own ADR.

## Consequences
- Remaining: sandboxes, the operator, sandbox-proxy, terminals, the chat tab, Claude credentials, GitHub login and App, push notification infrastructure, the updater, the chart, realtime and metrics.
- The step, session and run frames of the mux protocol, and the matching parts of sandbox-init, stay as generic sandbox primitives. No orchestrator drives them at the moment.
- The operator keeps its pending-change and drain logic. The drained-generation ack has no producer; rolls happen through the owner's Roll now (ADR-0023), idleness or the drain timeout.
- The sandbox NetworkPolicy no longer admits the executor pods or allows egress to them.
- Old data is dropped, not migrated: the chart's first migration Job (`legacy`, `receiver drop-legacy`, as the old owner role `enzarb_workflows_owner`) drops the four schemas, and the migration order is legacy, connectors, ui. The `connectors.installations` table is dropped. The `workflows.*` Helm values stay as deprecated, accepted and ignored keys.
- The separate workflow library repo is no longer referenced.
