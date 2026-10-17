# MVP: enzarb builds enzarb

## Goal
An enzarb deployment that works on its own repositories:
1. An issue opened in an `enzarb/*` repo (with the enzarb GitHub App installed) is triaged and planned by Claude in that Project's sandbox.
2. A maintainer comments `/approve` → Claude implements in a git worktree, pushes a branch, and enzarb opens a PR.
3. PRs get an automated review, and review comments are addressed.
4. The UI shows Projects, their boot checklist, work items and their history. It also offers a terminal into the sandbox for the one-time (per user, shared by all their Projects) `claude setup-token`.

**Done for this phase** = every component is built, tested in isolation, packaged as an image, and wired into the Helm chart, up to the point where deploying to a cluster has to be tested. Deployment testing is the next phase.

## In scope
- `proto/` v1: codec, flow control, session runtime (Rust), TS codec.
- `Project` CRD and operator: agent-sandbox `Sandbox`, sandbox auth Secret, volumes and NetworkPolicy in one shared namespace, status conditions (ADR-0013).
- `sandbox-init`: pid-1, mux server, PTYs, idempotent runs in git worktrees, Claude token capture.
- `connectors/`: trait, GitHub connector, webhook receiver.
- `workflows/`: engine (reducer, executors, ticker) and the `issue`, `change_request` and `mention` workflows.
- `sandbox-proxy`: authenticated websocket routing.
- `ui/`: GitHub login, Projects, work items, terminal, and the Merge action on `ReadyToMerge` change requests (the only way enzarb PRs get merged).
- `kubernetes/` prerequisites, the `helm/` chart, and the top-level `mise` dev tasks.
- CI for each repo.

## Deferred (post-MVP)
- `fresh` run mode and warm pools. MVP is `worktree` only; the CRD field exists.
- Kubernetes API access from sandboxes: none for now; when added, only to other namespaces (ADR-0013).
- Transcript object storage. Each step's transcript is stored in Postgres (`engine.session_events`, ADR-0022) and shown on the task page; `jobs.output` keeps only the summary.
- `app_api.identity_v` (enzarb user ↔ GitHub identity) for trigger authorization. MVP authorizes with `Connector::check_permission` (write access).
- Connectors other than GitHub, proxy HA/reconnect, gVisor/Kata (the RuntimeClass is configurable, unset on kind), observability beyond structured logs.

## Execution
- Tasks are GitHub issues in each component repo, in milestone **MVP**. The tracking issue in `enzarb/enzarb` lists them in dependency order.
- Each issue is implemented by one subagent on a branch, delivered as a PR with `Closes #N`, and merged after review.
- Shared interfaces are fixed in [contracts.md](contracts.md). An issue that needs to change a contract updates that doc in the same change.

### Waves
At most one task per repo at a time; each wave depends only on merged work from earlier waves.

| Wave | Tasks |
|---|---|
| 1 | proto codec · operator CRD types · connector trait · workflows schema · ui foundation · kubernetes prerequisites |
| 2 | proto session runtime + TS · operator controller · GitHub connector · workflows state machines · ui Projects |
| 3 | sandbox-init core · sandbox-proxy · webhook receiver · workflows reducer + ticker · ui work items |
| 4 | sandbox runs + token capture · workflows executor · ui terminal |
| 5 | sandbox image · operator packaging · workflows binary + packaging · helm chart · dev tasks |
