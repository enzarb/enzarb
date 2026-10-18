# 0025. Release timing: updater ConfigMap and Project `status.lastRoll`

Status: Accepted (2026-10-18).

## Context
We want merge-to-deployed time per chart release: merge, chart installed, sandbox applied.

## Decision
- **Merge:** the release `publishedAt` (already fetched by the UI), seconds after the merge.
- **Chart deployed:** the updater records the helm-install Job completion time when `installed` changes and persists the last 20 in the ConfigMap `enzarb-updater-releases`, served as additive `/status.releases`. The updater is DB-free (rejected: Postgres) and the `HelmChart` is owned by the cluster bootstrap with server-side apply (rejected: annotations on it).
- **Sandbox applied:** the operator writes `status.lastRoll {at, since, reason, image}` beside `pendingChange` (ADR-0023) when it deletes the pod; only the operator knows when that happens. No new write path, `pendingChange` is unchanged.
- **Join (UI):** by image tag when a release declares its sandbox image version, otherwise by time window between this release's deploy and the next. The chart does not expose the per-release sandbox image to the UI today, so the window is what runs. Several Projects roll at different times; the earliest is shown.

## Consequences
Only releases deployed, and rolls made, after this ships have data. The updater Role gains `create` on ConfigMaps. Everything is additive and rolls out in any order.
