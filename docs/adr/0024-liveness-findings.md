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
- **Platform facts** come from a provider trait. Wired: disk use from `sandbox_stats`, pending sandbox change. Chart deploy state, workflow run status and memory percentage have no source yet and are unknown.

## Alternatives rejected
- **A `platform` event into the reducer**: findings would pass through the interpreter and could cause transitions.
- **Reconcile writing `projection.reason/text`**: the reducer rewrites the projection on each transition, so the finding would flicker between passes, and "no side effects" would need an exception.
- **A scripting language for checks**: unreviewable and unbounded; a closed vocabulary lints and has a schema.

## Consequences
The change stream (`NOTIFY` on projection writes) does not push a raised or cleared finding; it appears on the next read or projection change. Production checks and UI rendering follow in the epic (#270).
