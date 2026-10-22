# enzarb monorepo

This is the **`enzarb`** monorepo (github.com/enzarb/enzarb, ADR-0015). Each component lives in its own top-level directory with its own build, checks, Dockerfile and release; `docs/` holds the architecture, contracts and ADRs shared by all of them.

Single-node RKE2 install on oldie (`enzarb.dev`): `docs/oldie.md` (`mise run oldie:up`; tasks in `.mise/tasks/oldie/`). CI and release automation: `docs/ci.md` (root `.github/workflows/`).

Local kind setup: `docs/dev.md` (`mise run dev:cluster|images|secrets|helm-deploy|webhook-relay|down`; tasks in `.mise/tasks/dev/`, shared helpers in `.mise/lib/dev.sh`).

Read `docs/architecture.md` first, then `docs/contracts.md` (the interfaces every component must match) and `docs/mvp.md` (current scope). Decisions live in `docs/adr/`, open questions in `docs/gaps.md`.

## Components

The Rust components form one Cargo workspace at the root (`Cargo.toml`, one `Cargo.lock`, one `rust-toolchain.toml`). Run each component's checks from the repo root with `-p` for its crates (single-crate components also work from inside their directory).

| Dir | What | Check / test |
|---|---|---|
| `proto/` | Mux protocol spec + Rust crate + TS package | `cargo test -p enzarb-proto`, `pnpm test` |
| `operator/` | `Project` CRD types crate + operator | `cargo clippy -p enzarb-crds -p enzarb-sandbox-operator --all-targets && cargo test -p enzarb-crds -p enzarb-sandbox-operator` |
| `connectors/` | GitHub webhook receiver that logs each delivery in `connectors.events` | `cargo clippy -p enzarb-receiver --all-targets && cargo test -p enzarb-receiver` |
| `realtime/` | Postgres `NOTIFY` (`connector_event`) to browser websocket change stream | `cargo clippy -p enzarb-realtime --all-targets && cargo test -p enzarb-realtime` |
| `sandbox-proxy/` | Browser websocket → sandbox router | `cargo clippy -p enzarb-sandbox-proxy --all-targets && cargo test -p enzarb-sandbox-proxy` |
| `sandbox-container/` | Sandbox image + pid-1 `sandbox-init` | `cargo test -p sandbox-init`; `docker build -f sandbox-container/Dockerfile .` (from the root) |
| `ui/` | SvelteKit app (remote functions = logic layer) | `pnpm run check && pnpm run lint && pnpm test` |
| `ez/` | enzarb's coding agent `ez` (ACP agent, TUI, `ez -p`; ADR-0028) | `cargo clippy -p ez-acp -p ez-claude -p ez-cli -p ez-core -p ez-hooks -p ez-llm -p ez-mcp -p ez-mise -p ez-tools -p ez-workflow --all-targets && cargo test -p ez-acp -p ez-claude -p ez-cli -p ez-core -p ez-hooks -p ez-llm -p ez-mcp -p ez-mise -p ez-tools -p ez-workflow` |
| `updater/` | Webhook service that patches the RKE2 `HelmChart` version on chart release | `cargo clippy -p enzarb-updater --all-targets && cargo test -p enzarb-updater` |
| `kubernetes/` | Cluster prerequisites (kind, Gateway, agent-sandbox, pgop; Capsule installed but unused, ADR-0013) | `kubectl kustomize` |
| `helm/` | Umbrella chart | `helm lint enzarb` |

## Working in the monorepo

- **Git hooks (opt-in):** run `mise run hooks:install` once per clone to get lefthook pre-commit/pre-push checks that mirror CI (`docs/dev.md`, "Git hooks"). Skip with `--no-verify` or `LEFTHOOK=0`.
- **One change, one PR**, even when it spans components. Change upstream code first in the same PR: `proto/` → `operator/` CRD types → `connectors/` → services (`realtime/`, `sandbox-proxy/`, `sandbox-container/`, `ui/`) → `helm/`.
- **Run the checks of every component you touch** (table above) before calling a change done.
- **Shared contracts:** the `proto/` frames, the `connectors.event_v` view and the `app_api` Postgres schema. Make additive changes, or add a new versioned object; never break a consumer silently (deployed components roll out independently).
- **One Cargo workspace.** New crates are added to `members` in the root `Cargo.toml`; do not add a `Cargo.lock`, `rust-toolchain.toml` or `[workspace]` in a component (`scripts/monorepo/check-cargo-deps.sh`). Dependencies are declared per crate. Select crates with `-p`: a bare root `cargo clippy` unifies features across all crates.
- **Cross-component deps are path deps** (`path = "../proto/crates/..."` in Cargo, `link:../proto` in `ui/package.json`). Docker builds of every Rust component and `ui` use the repo root as context: `docker build -f <dir>/Dockerfile .`.
- **Helm values:** never remove a values key in one release; deprecate it as an accepted, ignored key first (`docs/ci.md`, "Values compatibility"; `mise run compat` in `helm/` enforces it).
- **Releases:** PRs squash-merge with a Conventional Commit title (`feat(ui): …`, `fix(connectors): …`). Each component releases independently with tags `<dir>/vX.Y.Z` (e.g. `ui/v0.4.1`, `helm/v0.9.0`); image names are unchanged (`ghcr.io/enzarb/enzarb-ui`, …).
- **Dockerfiles** live in the directory of the component they build.
- **CI** follows `docs/ci.md`: per-component jobs gated by the generated `changes` job, rust-cache/npm/Playwright caches, cargo-chef + GHA layer cache, docker push on main.
- **Tooling comes from mise.** Workspace tools are pinned in the root `mise.toml`. A component that needs another tool adds it to its own `mise.toml` (`mise use <tool>@<version>`); no `curl | bash` or global installs.

## Conventions

- Rust edition 2024. Kubernetes types from kopium are committed, not generated at build time.
- UI: small components, one `global.scss`, no CSS frameworks; long operations show step-by-step checklists, never open-ended spinners.
- Scope: sandboxes against code repos, and logging the events those repos send (workflows and tasks were removed to be redesigned, ADR-0027). There is no `Environment` CRD and no deploy path (ADR-0006).
- Names: avoid words that already mean something in Kubernetes (ingress, gateway, router, proxy, controller, scheduler) for new components.
