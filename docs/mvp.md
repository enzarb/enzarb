# MVP: enzarb sandboxes for code repositories

The workflow engine, event hooks and tasks were removed to be redesigned ([ADR-0027](adr/0027-workflow-clean-slate.md)); this describes what the product is now.

## Goal
An enzarb deployment where people work with Claude in isolated sandboxes against their repositories:
1. The UI shows Projects and their boot checklist, and offers a terminal and a chat with the agent in the Project's sandbox. The terminal also runs the one-time (per user, shared by all their Projects) `claude setup-token`.
2. GitHub webhook deliveries for a Project's repositories are logged and listed live in the Project's Events tab.

**Done for this phase** = every component is built, tested in isolation, packaged as an image, and wired into the Helm chart, up to the point where deploying to a cluster has to be tested.

## In scope
- `proto/` v1: codec, flow control, session runtime (Rust), TS codec.
- `Project` CRD and operator: agent-sandbox `Sandbox`, sandbox auth Secret, volumes and NetworkPolicy in one shared namespace, status conditions (ADR-0013).
- `sandbox-init`: pid-1, mux server, PTYs, chats over ACP, generic step primitives, Claude token capture.
- `connectors/`: the webhook receiver that logs deliveries.
- `realtime/`: the change stream behind the live Events tab.
- `sandbox-proxy`: authenticated websocket routing.
- `ui/`: GitHub login, Projects, terminal, chat, Events, metrics.
- `kubernetes/` prerequisites, the `helm/` chart, and the top-level `mise` dev tasks.
- CI for each component.

## Deferred (post-MVP)
- Anything that acts on events or orchestrates agents (workflows, tasks): to be redesigned.
- `fresh` run mode and warm pools. MVP is `worktree` only; the CRD field exists.
- Kubernetes API access from sandboxes: none for now; when added, only to other namespaces (ADR-0013).
- Connectors other than GitHub, proxy HA/reconnect, gVisor/Kata (the RuntimeClass is configurable, unset on kind), observability beyond structured logs.

## Execution
- Work is tracked as GitHub issues in the `enzarb/enzarb` repo. Each is implemented on a branch and delivered as a PR with `Closes #N`, and merged after review.
- Shared interfaces are fixed in [contracts.md](contracts.md). An issue that needs to change a contract updates that doc in the same change.
