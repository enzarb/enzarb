# enzarb workspace

This directory is the top-level **`enzarb`** repository. It holds docs, ADRs, the repo manifest and cross-repo tasks. Each `enzarb-*/` subdirectory is a **separate git repository** listed in `repos.toml`. The root repo ignores them (`.gitignore`), and they are not submodules.

Read `docs/architecture.md` first, then `docs/contracts.md` (the interfaces every repo must match) and `docs/mvp.md` (current scope). Decisions live in `docs/adr/`, open questions in `docs/gaps.md`.

## Repos

| Repo | What | Check / test |
|---|---|---|
| `enzarb-proto` | Mux protocol spec + Rust crate + TS package | `cargo test`, `npm test` |
| `enzarb-sandbox-operator` | `Project` CRD types crate + operator | `cargo clippy --all-targets && cargo test` |
| `enzarb-connectors` | `Connector` trait, GitHub connector, webhook receiver | `cargo clippy --all-targets && cargo test` |
| `enzarb-workflows` | Postgres workflow engine (reducer, executors, ticker) | `cargo clippy --all-targets && cargo test` |
| `enzarb-sandbox-proxy` | Browser websocket → sandbox router | `cargo clippy --all-targets && cargo test` |
| `enzarb-sandbox-container` | Sandbox image + pid-1 `sandbox-init` | `cargo test`; `docker build .` |
| `enzarb-ui` | SvelteKit app (remote functions = logic layer) | `npm run check && npm run lint && npm test` |
| `enzarb-kubernetes` | Cluster prerequisites (kind, Gateway, Capsule, agent-sandbox, pgop) | `kubectl kustomize` |
| `enzarb-helm` | Umbrella chart | `helm lint enzarb` |

`mise run repos:list` shows which repos are present. `mise run repos:init` clones or initializes missing ones. `mise run repos:status` shows git status across all of them.

## Working across repos

- **Run git in the right repo.** Use `git -C enzarb-<name> …` and never commit component files to the root repo. Root commits are only for `docs/`, `repos.toml`, `mise.toml`, `CLAUDE.md` and `.claude/`.
- **One change, one branch name.** A change that spans repos uses the same branch name in every repo it touches, with one commit (or PR) per repo. Mention the related repos in each commit message.
- **Upstream first.** Change dependencies in order: `enzarb-proto` → operator CRD types → `enzarb-connectors` → services (`workflows`, `sandbox-proxy`, `sandbox-container`, `ui`) → `enzarb-helm`.
- **Run each repo's own checks** from inside that repo before calling a change done.
- **Shared contracts:** the `enzarb-proto` frames and the `workflows_api` / `app_api` Postgres schemas. Make additive changes, or add a new versioned object; never break a consumer silently.
- **Cross-repo Rust deps** are git dependencies on `main`, pinned by each repo's `Cargo.lock`. Merge library changes first, then run `cargo update -p <crate>` in the dependent repo. Never commit a `[patch]` to a local path.
- **Dockerfiles** live in the repo of the component they build.

## Conventions

- Rust edition 2024. Kubernetes types from kopium are committed, not generated at build time.
- UI: small components, one `global.scss`, no CSS frameworks; long operations show step-by-step checklists, never open-ended spinners.
- Scope: sandboxes, events and workflows against code repos. There is no `Environment` CRD and no deploy path (ADR-0006).
- Names: avoid words that already mean something in Kubernetes (ingress, gateway, router, proxy, controller, scheduler) for new components.
