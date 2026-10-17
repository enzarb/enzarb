## Summary

Replace the 8 independent Rust projects with one virtual workspace at the repo root. It has a single `Cargo.lock` and a single `rust-toolchain.toml` (`1.99.0`, rustfmt + clippy, the same as the 8 files today). No crate is renamed or moved, and image contents and release tags stay the same. Only build plumbing changes: manifests, Dockerfiles, CI, hooks, scripts, docs. This is one PR and is all-or-nothing.

## Approach

**Root manifest.** `Cargo.toml` is virtual, with `resolver = "3"`. Members are the 13 existing crates, listed explicitly (no globs):
- `proto/crates/enzarb-proto`
- `operator/crates/{enzarb-crds,operator}`
- `connectors/crates/{connector,connector-github}`, `connectors/bin/receiver`
- `workflows/crates/{engine,enzarb-workflows,flow}`
- `realtime`, `sandbox-container`, `sandbox-proxy`, `updater`

The `[workspace]` tables in the 4 multi-crate components go away. They cannot nest under a root workspace.

**Merging `[workspace.dependencies]` and `[workspace.package]`.** Today `proto`, `operator`, `workflows` and `connectors` each define their own table. Keys such as `serde`, `serde_json`, `thiserror`, `kube`, `k8s-openapi` and `tokio` collide, and their feature sets differ (`kube` is `derive` in operator but `client/runtime/rustls-tls/ring` in workflows).
- The root table holds each dep once with `default-features = false` and the version already in use.
- A member that needs features adds them next to `workspace = true`. Features are additive, so `kube = { workspace = true, features = [...] }` reproduces the old set.
- `version`, `edition`, `license`, `publish` and `repository` differ across components (`UNLICENSED` for proto, `Apache-2.0` elsewhere). I will not unify them. Each member keeps its own `[package]` values and only `edition` moves to `edition.workspace = true`. This avoids a licence or publish change nobody asked for.
- The standalone crates (`realtime`, `updater`, `sandbox-proxy`, `sandbox-container`) keep their inline deps. Only the path deps (`../proto/...`) need the same relative paths adjusted where they change.

**Lockfile.** To keep versions stable, I will start from a root `Cargo.lock` produced by merging the existing lockfiles. Then I run `cargo generate-lockfile` and check which versions moved. I will `cargo update -p <crate> --precise <old>` for anything that moved without a reason, such as `kube`, `k8s-openapi`, `sqlx`, `tokio` and `axum`. Where two components need incompatible majors, both stay in the lock. For example `base64` is 0.22 in sandbox-proxy and 0.23 in sandbox-container, and `sha2` is 0.10 vs 0.11. The PR description will list the version deltas.

**Feature unification.** In a workspace, `cargo build -p X` unifies features across only X's dependency graph (resolver 2/3), so per-package builds behave as before. A bare `cargo clippy` at the root would unify across everything, so it is not the documented check.
- The documented check stays "from the component directory". Inside a member directory cargo targets that package. In the multi-crate components (no root package) it needs `--workspace` or explicit `-p` flags.
- Cargo applies `--workspace` to the whole root workspace, not to the component. So `cargo clippy --workspace` in `workflows/` would check all 13 crates.
- I therefore change every multi-crate command to explicit `-p` lists. CI and hooks use `-p` for those components, and the single-crate ones keep running from their directory.
- Alternative rejected: a root-level `cargo clippy --workspace` as the single check. It loses per-component path filtering and blows up job time and cache size.

**Toolchain, `.cargo/config.toml`, lint settings.** The root `rust-toolchain.toml` replaces the 8 files. The comment in `.cargo/config.toml` ("each repo keeps its own target/") becomes stale, since there is now one `target/` at the root. I will update the comment and keep `git-fetch-with-cli`. Profiles: none exist today, so nothing to move.

**Dockerfiles (cargo-chef).** Every Rust image gets the repo root as build context, and `docker build -f <dir>/Dockerfile .` is unchanged. Today `operator`, `realtime` and `updater` build with their own directory as context, so they switch to the root and their `COPY` lines are prefixed. `realtime` and `updater` also switch their CI `context` to `.`.

Each build must resolve the whole workspace, so the planner/build stages copy every member manifest. cargo-chef's `prepare` writes a workspace-wide skeleton, so the recipe has to be scoped. The design:
1. `planner`: `COPY` the root `Cargo.toml`, `Cargo.lock` and `rust-toolchain.toml`. Also `COPY` all member sources (a tiny `.dockerignore`-friendly set) and run `cargo chef prepare --recipe-path recipe.json`.
2. `build`: `COPY` the recipe and run `cargo chef cook --release --locked -p <crate>`. Only that crate's dependency graph is cooked.
3. `COPY` the real sources of the crate and its path deps, then `cargo build --release --locked -p <crate>`.

The recipe JSON changes whenever any member's manifest changes. In practice the cook layer is keyed on the recipe content, so a dependency change in an unrelated crate invalidates the cook layer for every image. To limit this, I will try the following and measure which one works:
- **(a)** Each Dockerfile copies only the manifests of that image's transitive path-dep members and removes the other members from a copy of the root `Cargo.toml`. This is brittle.
- **(b)** Accept the invalidation on manifest changes only. Source-only changes do not alter the recipe, so the common case still hits the cache.

I propose **(b)** first. The workspace-wide dependency churn then happens only when someone edits a `Cargo.toml` or `Cargo.lock`, and (a) is a follow-up if the numbers say otherwise. This is the open question below.

`sandbox-container` keeps its tooling/runtime stages untouched; only the Rust build stage changes. `workflows` and `connectors` keep the `flow` skeleton workaround, adapted to the new paths (`COPY workflows/crates/flow ...` before the final build). `.dockerignore` files in `operator/`, `connectors/`, `sandbox-container/` and `workflows/` are per-directory and only apply when that directory is the context. They must be merged into the root `.dockerignore` where that matters (for example `sandbox-container`'s `tests` exclusion).

**CI.**
- `_rust.yml`: `setup-rust-toolchain` gets `rust-src-dir: .`. rust-cache uses `workspaces: . -> target` with a per-component key. fmt still uses `working-directory`. clippy/nextest/doctest take an explicit `cargo-args` made of `-p` flags, so the shared `target/` does not mix components. Add a root `Cargo.lock` hash to the cache key via rust-cache's default (it hashes `Cargo.lock` at the workspace root).
- `ci-*.yml`: for every Rust component, add `Cargo.toml`, `Cargo.lock` and `rust-toolchain.toml` to the `paths:` filters. Replace `cargo-args` values with the `-p` list for each component (and its path-dep crates where tests should run).
- `ci-proto.yml`: update the `rust-src-dir` usage and its own working-directory handling.
- `rebuild-all.yml`, `sync-components.yml`, `release.yml` and `image-components.sh` use tags and component dirs, not lockfiles. I will confirm by grep that none reads `<dir>/Cargo.lock`, and fix any that do.
- `docs/ci.md` says the "Toolchain file per component" and "pin an exact version" rules apply. I will rewrite them as "one root toolchain file".

**Tooling scripts.**
- `scripts/monorepo/affected.sh` and `test-affected.sh`: add `Cargo.toml Cargo.lock rust-toolchain.toml` to each Rust component's RULES row (and keep it in sync with the `ci-*.yml` `paths:` filters, which `test-affected.sh` enforces). The root files then trigger all 8 Rust components, and the existing "root files trigger nothing" comment is updated to say Cargo files are an exception.
- `check-cargo-deps.sh`: the grep already scans recursively and still works. I will add checks that exactly one `Cargo.lock` and one `rust-toolchain.toml` exist, that no member has its own `[workspace]`, and that every `Cargo.toml` is a root member (via `cargo metadata --no-deps --format-version 1`). Lefthook's glob already matches the root files.
- `pre-push.sh` and `ci-local.sh`: switch the per-component commands to the `-p` form and `rust-src-dir`/toolchain handling. `lefthook.yml`: the 8 `fmt-*` commands become one `cargo fmt --all --check` at the root, gated on staged `*.rs`.
- `mechanical.sh` (and `test-mechanical.sh`): `Cargo.lock | */Cargo.lock` already includes the root lockfile. Verify only.
- `mise.toml`: the per-component `mise.toml` files that pin `cargo-nextest` stay. The root keeps Rust pinned through `rust-toolchain.toml`. I will check that no component `mise.toml` pins a `rust` version.

**Docs.** `CLAUDE.md` (component table and the "run each component's checks from inside its directory" wording), `docs/ci.md`, `docs/dev.md`, `docs/contracts.md:27` (toolchain line), `sandbox-container/README.md` (examples only, probably unchanged).

## Changes

- Add: `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`.
- Delete: 8 `Cargo.lock` and 8 `rust-toolchain.toml`. Remove the `[workspace*]` tables from `connectors`, `operator`, `proto` and `workflows` `Cargo.toml`, and rewrite member manifests to the merged `workspace = true` deps (`connectors/crates/*`, `connectors/bin/receiver`, `operator/crates/*`, `proto/crates/enzarb-proto`, `workflows/crates/*`).
- Each directory's `Cargo.toml` is a `[package]` manifest only where there is a single crate (`realtime`, `sandbox-container`, `sandbox-proxy`, `updater`). In the four multi-crate components, the dir-level `Cargo.toml` is deleted because there is no package there.
- Dockerfiles (7): `connectors`, `operator`, `realtime`, `sandbox-container`, `sandbox-proxy`, `updater`, `workflows`. `.dockerignore` merge.
- CI: `_rust.yml`, `ci-{connectors,operator,proto,realtime,sandbox-container,sandbox-proxy,updater,workflows}.yml`. Also `ci-ui.yml` if its path filter lists `proto/**` Cargo files, which it should not need.
- Tooling: `lefthook.yml`, `scripts/monorepo/{check-cargo-deps,affected,test-affected,ci-local,pre-push}.sh`, `.cargo/config.toml`.
- Docs: `CLAUDE.md`, `docs/ci.md`, `docs/dev.md`, `docs/contracts.md`.

## Tests

Run locally (everything that does not need network or Docker daemon access beyond what the sandbox has):
- `cargo metadata --format-version 1 --no-deps` at the root: all 13 crates present, plus `find . -name Cargo.lock -o -name rust-toolchain.toml | grep -v target` yields exactly one of each.
- For each component, the documented check with `-p` flags, for example:
  - `cargo clippy -p enzarb-proto --all-targets -- -D warnings && cargo test -p enzarb-proto`
  - `cargo clippy -p enzarb-crds -p enzarb-sandbox-operator --all-targets --all-features -- -D warnings && cargo test ...`
  - the same for connectors (3 crates), workflows (3 crates), realtime, sandbox-proxy, sandbox-container (`--locked`) and updater.
- The same commands run from inside each component directory, to confirm both forms work.
- `cargo fmt --all --check`, `cargo build --locked` (confirms the lockfile is complete), and `cargo tree -d` to review duplicate versions.
- `scripts/monorepo/check-cargo-deps.sh`, `scripts/monorepo/test-affected.sh`, `test-mechanical.sh`, `shellcheck`, and `actionlint` on the edited workflows.
- `docker build -f <dir>/Dockerfile .` for all 7 images. If Docker or network is unavailable in the implementation environment, I will say so in the PR description and rely on CI for them.
- Tests for the new `check-cargo-deps.sh` rules and the new `affected.sh` rows (extend `test-affected.sh` with a root `Cargo.lock` case that selects all 8 Rust components).

CI cache behavior cannot be verified locally. I will check the Actions logs on the PR, including a second run to see cache hits.

## Risks and open questions

1. **Feature unification inside the merged workspace table.** Merging the dep tables is the part most likely to change behavior. Mitigation: a `cargo tree -e features -p <crate>` before/after comparison for each binary crate. Binary crates feed the images, so these must match.
2. **Lockfile bumps.** See Approach. A small number of incidental bumps may be unavoidable; the PR will list them.
3. **cargo-chef recipe scope** (option (a) vs (b) above). I need a decision if you prefer strict per-image cache isolation now rather than as a follow-up.
4. **Shared `target/` in CI.** One workspace and one `target/` dir means jobs for different components might cache different subsets under the same path. Cache keys stay per component and job to avoid cross-contamination, but the first runs on main will be cold.
5. **Disk and build time for `sandbox-container` and `connectors` Docker builds.** Their context grows to the repo root. `.dockerignore` already excludes `ui/node_modules`, docs, helm and kubernetes.
6. **Per-component `.dockerignore` files** stop applying once the context is the root, so they are merged into the root file.
7. **Dependabot / "bump" flows.** The `fix(deps): bump ...` commits come from an enzarb-internal flow that I could not find in `.github/` (there is no `dependabot.yml`). If it targets `<dir>/Cargo.lock`, it needs a path update. I will grep once more and flag if found.
8. **Assumption:** the repo-local `.cargo/config.toml` sits above the old per-repo roots and is still picked up unchanged.
9. The `license`, `version` and `publish` fields stay per-crate (not unified).

## Out of scope

- Renaming crates or moving component directories.
- Unifying versions, licences or `publish` settings across crates.
- Dependency upgrades beyond what lockfile unification forces.
- Per-image cargo-chef recipe isolation beyond what option (b) gives (follow-up if cache numbers justify it).
- Changing release tags, the Helm chart, or the UI package setup.

Reply with feedback to revise this plan, or comment /approve to start the implementation.
