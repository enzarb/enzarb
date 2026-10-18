# 0024. Liveness findings: pure evaluation, a periodic `platform` source, observation only

Status: Accepted (2026-10-18).

## Context
A task can wait for something that will not come (CI that never started, a fact that stays unknown), and the platform itself can be unhealthy (full sandbox disk, a stuck chart deploy). Nothing said so. The engine kernel must stay free of workflow knowledge, and noticing a problem must never change what a task does.

## Decision
- **Finding.** `{key, kind: dead_wait|platform_failure, severity: info|warning|critical, text}`, stored in `engine.findings` (one row per key; cleared rows are kept).
- **Checks are data.** `checks/*.yaml` in the workflow library: `all`/`any`/`not` over a closed set of leaves (task facts, `fact_unknown_for`, `fact_count`, `idle_for`, platform facts). No scripting; CI and GitHub appear only in library data. Thresholds live only there; the kernel ships no defaults.
- **`platform` is an internal source**: a ticker pass every `ENZARB_FINDINGS_SECS` (60). It has no task-event payload and no entry in the event catalog.
- **Pure evaluate, then reconcile.** `evaluate(checks, snapshot, now)` has no I/O. `reconcile` is the only writer and writes `engine.findings` only; `engine.facts.unknown_refreshes` is kept by the fact refresh. Tasks, projection, timers, jobs, step runs, events and transitions are never written, so "no side effects on tasks" holds by construction.
- **Lifecycle.** Raised on the second consecutive pass that wants it (no flapping), refreshed while it holds, cleared when it stops or the task ends. A check that depends on an unknown platform fact is undetermined: nothing is raised and an open finding is left alone (a missing source is not a failure). Keys are per `(check, task)` or `(check, project)`, so a stuck task yields one finding per check however many passes see it.
- **Read-path projection.** `task_v.reason/text` are `coalesce(interpreter reason, top open finding)` beside `task_v.findings`; `finding_v` lists every open finding for a platform view. Interpreter reasons always win.
- **Observation only.** A finding never cancels, stops, restarts, nudges or times out anything and sends no notification.
- **Platform facts** come from a provider trait. Wired: disk and memory use from `sandbox_stats` (memory needs the cgroup `memory.max` the stats frame now carries), pending sandbox change, the latest workflow run through `Connector::latest_workflow_run`, and chart deploy state from the updater's `/status` (`ENZARB_UPDATER_STATUS_URL`). Connector and updater answers are cached for 30 s, so each pass asks once. A source that is absent or fails gives an unknown fact. The library checks (failed `release.yml`/`rebuild-all`, undeployed chart, sandbox disk and memory, unapplied change) are data in `checks/*.yaml`.

## Alternatives rejected
- **A `platform` event into the reducer**: findings would pass through the interpreter and could cause transitions.
- **Reconcile writing `projection.reason/text`**: the reducer rewrites the projection on each transition, so the finding would flicker between passes, and "no side effects" would need an exception.
- **A scripting language for checks**: unreviewable and unbounded; a closed vocabulary lints and has a schema.

## Consequences
The change stream (`NOTIFY` on projection writes) does not push a raised or cleared finding; it appears on the next read or projection change. Production checks and UI rendering follow in the epic (#270).

## Addendum: a finding may file one issue (#280)
`reconcile` still never touches tasks, the projection, timers, step runs or transitions, but it may now also enqueue one `file_issue` job per newly opened finding whose check has an `issue:` block (`{title, body, labels?, repo?}`, any kind and scope). The job is inserted in the finding's transaction, keyed `finding:<check>:<subject>` (task id, or project for platform scope), and runs the existing `file_issue` action, which upserts by that key. The key carries no time, so a finding that clears and returns files no second issue. A library action node reacting to a finding was rejected: it would let findings drive transitions. `engine.jobs.task_id` becomes nullable (additive) for platform findings. Findings stay observation only; nothing is cancelled or restarted, and the issue is not closed when the finding clears.

## Addendum: no issue about an issue a finding filed (#324)
A finding on a task whose own source issue was filed by a finding (the stored issue body carries the `file_issue` marker, `has_file_issue_marker`) is still recorded and shown, but no `file_issue` job is queued. Without this the filed issue starts a task, that task matches a check (e.g. `external-idle`), and the check files another issue without end; the key only deduplicates per check and task. The guard is per task, covers every check, and needs no YAML or schema change.
