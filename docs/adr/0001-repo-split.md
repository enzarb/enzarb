# 0001. One repo per component, plus a top-level `enzarb` repo

## Context
The components ship separately (UI, proxy, sandbox image, operator, connectors, workflow engine, chart) and run on different schedules. There were no commits yet, so the split cost nothing.

## Decision
- One git repo per component, listed in [architecture.md](../architecture.md#repositories).
- The top-level `enzarb` repo holds docs, ADRs, `repos.toml` (the repo manifest) and cross-repo `mise` tasks (`repos:*`, `dev:helm-deploy`).
- Shared code is published as libraries, not copied:
  - `enzarb-proto` (Rust crate + npm package);
  - `enzarb-connectors` library crates;
  - the operator's CRD types crate.
- Dockerfiles live in the repo of the component they build.

## Consequences
- Changes to `enzarb-proto` or `workflows_api` that cross repos need versioning discipline: semver for the libraries, and for the API schema, additive changes or a new versioned view/function.
- Local development needs every repo checked out side by side; `mise run repos:init` handles that.
