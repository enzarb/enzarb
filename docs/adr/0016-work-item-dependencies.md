# 0016. Work item dependencies

## Context
Issues and PRs often must wait for other work. Without a model for that, a PR whose prerequisite is unmerged can reach `ReadyToMerge` and be merged out of order.

## Decision
- **Where they are declared.** GitHub's native "blocked by" relationships (read through `Connector::list_dependencies`, written through `add_dependency` / `remove_dependency`), and `Depends on #N` / `Blocked by #N` / `Depends on owner/repo#N` text in a body or comment (`enzarb_connector::dependencies`, a pure parser: case-insensitive, lists, no code blocks or quoted lines, no prose such as "depends on the #1 priority").
- **Storage.** `workflows.work_item_dependencies(work_item_key, depends_on_key, source, created_at)`, `source` one of `github`, `text` (body) or `comment`. Each source replaces only its own rows (a comment only adds), so sources never overwrite each other and cannot loop. A key with no work item yet is `pending`. Keys, not ids, because a dependency can be declared before either item has a row. Only dependencies inside one Project are stored; others are ignored and logged. A self dependency is dropped.
- **Inheritance.** A change request inherits the dependencies of the issue its body closes (`Closes|Fixes|Resolves #N`). It is computed in the view `workflows.effective_dependencies`, never copied.
- **Done.** A dependency is done when its work item is `Done` (issue) or `Merged` (change request). `Cancelled`, `Closed` and `Duplicate` are *closed without completing*: the dependent stays blocked with that reason, and a maintainer removes the dependency. A missing item stays `pending`.
- **Merge gate.** `Readiness` has a fourth check, `dependencies`. `all_ok` requires it; `needs_check` ignores it, so it never calls the connector. The reducer reads the open set into `Ctx::open_dependencies` for every event, so the merge action sees it fresh. `CrData.open_dependencies` stores it for the views (`work_item_v.blocked_by_dependency`, `state_data.checklist.dependencies`).
- **Auto-continue.** The ticker sweeps change requests, and emits `DependenciesChanged` when the open set differs from the stored one; the machine re-runs `evaluate`. A missed webhook delays this by one sweep.
- **Cycles.** `workflows.dependency_cycle(key)` (recursive CTE) finds a shortest cycle through an item. Members get an open dependency whose problem is `dependency cycle: #1 → #2 → #1`; the merge gate stays closed.
- **GitHub sync.** The ticker lists the native relationships of active items every two minutes and replaces the `github` rows. A connector without the capability, or a token without access, is skipped: the feature degrades to text-only.

## Consequences
- Shared contracts stay additive: a table, functions, views, one trailing column on `work_item_v`.
- Closing an issue as completed without a PR does not mark it `Done` (the issue machine treats any close as `Cancelled`); a dependency on such an issue reads as closed without completing until the dependency is removed.
- Not yet built: the start gate (a per-Project setting that holds implementation), the planning prompt block, and the UI. They follow in separate PRs.
