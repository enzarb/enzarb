# enzarb monorepo

This is the **`enzarb`** monorepo (github.com/enzarb/enzarb, ADR-0015). Each component lives in its own top-level directory with its own build, checks, Dockerfile and release; `docs/` holds the architecture, contracts and ADRs shared by all of them.

Single-node RKE2 install on oldie (`enzarb.dev`): `docs/oldie.md` (`mise run oldie:up`; tasks in `.mise/tasks/oldie/`). CI and release automation: `docs/ci.md` (root `.github/workflows/`).

Local kind setup: `docs/dev.md` (`mise run dev:cluster|images|secrets|helm-deploy|webhook-relay|down`; tasks in `.mise/tasks/dev/`, shared helpers in `.mise/lib/dev.sh`).

Read `docs/architecture.md` first, then `docs/contracts.md` (the interfaces every component must match) and `docs/mvp.md` (current scope). Decisions live in `docs/adr/`, open questions in `docs/gaps.md`.

## Components

Run each component's checks from inside its directory.

| Dir | What | Check / test |
|---|---|---|
| `proto/` | Mux protocol spec + Rust crate + TS package | `cargo test`, `npm test` |
| `operator/` | `Project` CRD types crate + operator | `cargo clippy --all-targets && cargo test` |
| `connectors/` | `Connector` trait, GitHub connector, webhook receiver | `cargo clippy --all-targets && cargo test` |
| `workflows/` | Postgres workflow engine (reducer, executors, ticker) | `cargo clippy --all-targets && cargo test` |
| `realtime/` | Postgres `NOTIFY` to browser websocket change stream | `cargo clippy --all-targets && cargo test` |
| `sandbox-proxy/` | Browser websocket → sandbox router | `cargo clippy --all-targets && cargo test` |
| `sandbox-container/` | Sandbox image + pid-1 `sandbox-init` | `cargo test`; `docker build -f sandbox-container/Dockerfile .` (from the root) |
| `ui/` | SvelteKit app (remote functions = logic layer) | `npm run check && npm run lint && npm test` |
| `updater/` | Webhook service that patches the RKE2 `HelmChart` version on chart release | `cargo clippy --all-targets && cargo test` |
| `kubernetes/` | Cluster prerequisites (kind, Gateway, agent-sandbox, pgop; Capsule installed but unused, ADR-0013) | `kubectl kustomize` |
| `helm/` | Umbrella chart | `helm lint enzarb` |

## Working in the monorepo

- **One change, one PR**, even when it spans components. Change upstream code first in the same PR: `proto/` → `operator/` CRD types → `connectors/` → services (`workflows/`, `sandbox-proxy/`, `sandbox-container/`, `ui/`) → `helm/`.
- **Run the checks of every component you touch** from inside its directory before calling a change done.
- **Shared contracts:** the `proto/` frames and the `workflows_api` / `app_api` Postgres schemas. Make additive changes, or add a new versioned object; never break a consumer silently (deployed components roll out independently).
- **Cross-component deps are path deps** (`path = "../proto/crates/..."` in Cargo, `file:../proto` in `ui/package.json`). Docker builds for components with path deps (`ui`, `sandbox-proxy`, `sandbox-container`, `workflows`) use the repo root as context: `docker build -f <dir>/Dockerfile .`.
- **Helm values:** never remove a values key in one release; deprecate it as an accepted, ignored key first (`docs/ci.md`, "Values compatibility"; `mise run compat` in `helm/` enforces it).
- **Releases:** PRs squash-merge with a Conventional Commit title (`feat(ui): …`, `fix(workflows): …`). Each component releases independently with tags `<dir>/vX.Y.Z` (e.g. `ui/v0.4.1`, `helm/v0.9.0`); image names are unchanged (`ghcr.io/enzarb/enzarb-ui`, …).
- **Dockerfiles** live in the directory of the component they build.
- **CI** follows `docs/ci.md`: per-component path-filtered jobs, rust-cache/npm/Playwright caches, cargo-chef + GHA layer cache, docker push on main.
- **Tooling comes from mise.** Workspace tools are pinned in the root `mise.toml`. A component that needs another tool adds it to its own `mise.toml` (`mise use <tool>@<version>`); no `curl | bash` or global installs.

## Conventions

- Rust edition 2024. Kubernetes types from kopium are committed, not generated at build time.
- UI: small components, one `global.scss`, no CSS frameworks; long operations show step-by-step checklists, never open-ended spinners.
- Scope: sandboxes, events and workflows against code repos. There is no `Environment` CRD and no deploy path (ADR-0006).
- Names: avoid words that already mean something in Kubernetes (ingress, gateway, router, proxy, controller, scheduler) for new components.
