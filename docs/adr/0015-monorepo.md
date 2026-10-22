# 0015. Monorepo

Supersedes [ADR-0001](0001-repo-split.md) (one repo per component).

## Context
ADR-0001 split enzarb into eleven component repos plus a top-level docs repo. In practice most changes crossed repos (proto → operator CRD types → connectors → services → chart), each needing its own branch, PR, release and `cargo update -p` in every consumer. Cross-repo git dependencies forced read-only deploy keys, ssh build secrets in every Dockerfile, an `@enzarb/proto` package on GitHub Packages and a chart-sync workflow that dispatched across repos with a PAT. Agents working an issue saw one repo at a time and could not land an atomic change.

## Decision
- **One repo, github.com/enzarb/enzarb.** Each component is a top-level directory: `proto`, `operator`, `connectors`, `realtime`, `sandbox-proxy`, `sandbox-container`, `ui`, `updater`, `kubernetes`, `helm`. `docs/`, ADRs and `mise` tasks stay at the root; `repos.toml` and the `repos:*` tasks are gone.
- **History is kept.** Each old repo was imported with `git filter-repo --to-subdirectory-filter <dir>` and its tags renamed with a `<dir>/` prefix (`v0.4.1` → `ui/v0.4.1`), then merged into the root history.
- **Per-component CI and release.** Workflows are path-filtered per directory. PRs squash-merge with a Conventional Commit title; each component releases independently with tags `<dir>/vX.Y.Z`. Image names are unchanged (`ghcr.io/enzarb/enzarb-ui`, …), and so is the chart (`oci://ghcr.io/enzarb/charts/enzarb`).
- **In-repo chart sync.** A component release bumps its image tag in `helm/` within the same repo; a `helm/vX.Y.Z` release publishes the chart. The updater acts only on `helm/v*` releases of `enzarb/enzarb`.
- **Path dependencies.** Rust crates depend on siblings with `path = "../proto/crates/enzarb-proto"` etc.; the UI depends on `@enzarb/proto` via `file:../proto`. Images with path deps (`ui`, `sandbox-proxy`, `sandbox-container`) build from the repo root: `docker build -f <dir>/Dockerfile .`. No deploy keys or build secrets.

## Consequences
- A change that spans components is one branch and one PR; contract changes still stay backward compatible because deployed components roll out independently.
- The old `enzarb/enzarb-*` repos are archived (read-only). Issue and PR links to them (`enzarb-ui#66`) stay valid as history.
- GHCR packages created by the old repos must grant `enzarb/enzarb` write access (package settings → Manage Actions access) before the monorepo's CI can push to them.
- The `ENZARB_*_DEPLOY_KEY` secrets, the `@enzarb/proto` GitHub Packages release and the cross-repo chart-sync dispatch are obsolete ([contracts.md §11](../contracts.md#11-ci-access-to-private-enzarb-dependencies-obsolete)).
- The About page reads releases from `enzarb/enzarb` and filters them by tag prefix.
