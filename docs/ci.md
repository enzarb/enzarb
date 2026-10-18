# CI conventions

The monorepo's GitHub Actions setup (root `.github/`) follows these rules. Each component has its own `ci-<dir>.yml` with no `paths:` filter: it runs on every push to `main` and pull request, and its first job (`changes`, the reusable `_changes.yml`) decides from the generated dependency graph whether the component or one of its path dependencies changed. Every other job `needs: changes` and runs only then, so a change only builds what it can affect and the others report as skipped. Rust components share the reusable `_rust.yml`; image builds share `_docker.yml`. The goals are fast feedback on PRs, building each artifact once, and caching that persists between runs.

## All workflows
- **Cancel superseded runs:**
  ```yaml
  concurrency:
    group: ${{ github.workflow }}-${{ github.ref }}
    cancel-in-progress: ${{ github.event_name == 'pull_request' }}
  ```
- **Least privilege:** set `permissions` explicitly per workflow or job.
- **Timeouts:** every job has `timeout-minutes`.
- **Parallel by default:** independent checks are separate jobs with no `needs:`. Use `needs:` only for real data dependencies, such as pushing an image after its smoke test.
- **Tools:** come from `jdx/mise-action` with `cache: true`, reading the root `mise.toml`. No `curl | bash` installers. See "Tool pins" below.

## Rust repos
- **One workspace:** the root `Cargo.toml` is a virtual workspace with every crate as a member, one `Cargo.lock` and one `rust-toolchain.toml`. Per-component `Cargo.lock`/`rust-toolchain.toml` files and nested `[workspace]` tables are not allowed (`scripts/monorepo/check-cargo-deps.sh` enforces this). Dependencies stay declared per crate, so each component keeps its own feature set. A change to the root `Cargo.toml`, `Cargo.lock` or `rust-toolchain.toml` triggers every Rust component. Select a component's crates with `-p` flags: a bare `cargo clippy` at the root would unify features across all 13 crates.
- **Toolchain:** pin an exact version in the root `rust-toolchain.toml` (e.g. `channel = "1.99.0"`), never `stable`. Runners resolve `stable` to different versions, which changes rust-cache's environment hash so every run misses. Install it via `actions-rust-lang/setup-rust-toolchain` with its built-in cache off (`cache: false`); we use rust-cache.
- **Separate, parallel jobs:**
  - `fmt`: `cargo fmt --all --check` plus the Cargo guard (`check-cargo-deps.sh`): no `[patch]`, no git dependency on an enzarb repo (enzarb crates are path deps), one lockfile and toolchain file. No cache, no deps fetch.
  - `clippy`: `cargo clippy --all-targets <cargo-args> -- -D warnings`, where `cargo-args` is the `-p` list of the component (plus `--all-features` where set).
  - `test`: `cargo nextest run` (installed with `taiki-e/install-action@nextest`) plus `cargo test --doc` if the crate has doctests. Integration tests that need Docker run here.
- **Cache:** `Swatinem/rust-cache@v2` in clippy and test.
  - `workspaces: . -> target` (the single root `target/`) and a distinct `key` per component and job (`<dir>-clippy`, `<dir>-test`), because clippy and test build different artifacts.
  - Set `save-if: ${{ github.ref == 'refs/heads/main' }}`. PRs read the main cache but don't write, so cache churn stays low.
- **Toolchain file:** `setup-rust-toolchain` gets `rust-src-dir: .`; jobs run at the repo root and select crates with `-p` (the `cargo-args` input).

## Tool pins
The root `mise.toml` is the only place tool versions are pinned (concrete versions, never `latest`/`lts`); component `mise.toml` files hold tasks only. Ansible is the exception in placement only: `mise.ansible.toml` at the root, activated with `MISE_ENV=ansible` (the `ci-kubernetes.yml` ansible job, `oldie:host`, `oldie:reset`), so its pipx builds do not slow every task. The buildx pin is used by `ci-release-path.yml`, which installs that binary as the docker CLI plugin (the release workflows keep `docker/setup-buildx-action`). `setup-node` cannot read `mise.toml`, so jobs use the `.github/actions/setup-node` composite action, which reads the `node` pin.

`scripts/monorepo/check-tool-pins.sh` (job `scripts` in `ci-monorepo.yml`, and the pre-commit hook) fails when:
- a component `mise.toml` has a `[tools]` section, or a root pin is `latest`/`lts`;
- a Dockerfile `ARG` tool version differs from the root pin (mapping table at the top of the script: `NODE_VERSION` to `node`, `GH_VERSION` to `gh`, `BUILDX_VERSION` to `github:docker/buildx`; `PLAYWRIGHT_VERSION` is compared with `ui/package-lock.json`);
- an `ARG *_VERSION` is in neither the mapped nor the explicitly unmapped list (docker, compose, sccache, mise, claude-agent-acp, gVisor), so the table cannot go stale;
- root `rust` differs from a `rust-toolchain.toml` channel, or `HELM_VERSION` in `kubernetes/hack/versions.env` differs from root `helm`.

`scripts/monorepo/test-tool-pins.sh` tests the check against fixtures. To bump a tool, change the root pin and the matching Dockerfile `ARG` in the same PR.

## Retag dry run on release-path PRs
`ci-release-path.yml` runs the retag step of the release path in dry-run mode against real registry images, so a change that breaks it (for example dropping `--prefer-index=false`) fails the PR instead of the next release.

- **Gating:** the `release-path` pseudo-component in `scripts/monorepo/components.json` (`.github/scripts/**`, `release.yml`, `_release-*.yml`, `rebuild-all.yml`, `ci-release-path.yml`, the graph files and their generator, `test-retag-or-build.sh`, `mise.toml`). Other PRs skip the job through `_changes.yml`.
- **Forks:** PRs from forks have no registry access; the job runs but every step is skipped with a note in the summary, so it does not fail.
- **Source image:** the first of the last 20 `main` commits and the images from `image-components.sh` for which `retag-or-build.sh check` says `retag` (the `sha-` image exists with a matching revision label). One image is enough, the code path does not depend on it; the summary shows which manifest type it was. If none qualifies, the job notes that and passes (the release would rebuild).
- **Check:** `retag-or-build.sh verify` (also `DRY_RUN=true retag-or-build.sh retag`) runs the same `docker buildx imagetools create` as the real retag, shared in one function, with `--dry-run`. It hashes the manifest the command prints and fails unless its media type and digest equal the source's, i.e. a single-manifest source is not wrapped in an index. The summary lists source image, manifest type and digest.
- **Buildx:** the version pinned in the root `mise.toml` (see "Tool pins").

## Git hooks
`lefthook.yml` (opt-in, `mise run hooks:install`, see `docs/dev.md`) runs the same checks locally. Keep the two in sync when you change either.

| Hook | Command | CI job |
|---|---|---|
| pre-commit | `cargo fmt --all --check` at the root | `_rust.yml` `fmt` |
| pre-commit | `scripts/monorepo/check-cargo-deps.sh` (shared script) | `_rust.yml` `fmt` |
| pre-commit | `scripts/monorepo/check-tool-pins.sh` (shared script) | `ci-monorepo.yml` `scripts` |
| pre-commit | `scripts/monorepo/gen-graph.sh --check` when a `Cargo.toml`, `package.json` or the graph manifest is staged | `ci-monorepo.yml` |
| pre-commit | `npm run lint` in `ui/` | `ci-ui.yml` `check` |
| pre-commit | `shellcheck` on staged scripts | (no dedicated job) |
| pre-commit | `mise run lint` in `helm/` | `ci-helm.yml` |
| pre-push | `cargo clippy --all-targets <cargo-args> -- -D warnings` | `_rust.yml` `clippy` |
| pre-push | `cargo test <cargo-args>` | `_rust.yml` `test` (CI uses nextest plus `cargo test --doc`; the hook runs plain `cargo test`, which includes doctests) |
| pre-push | `mise run proto:gen -- --check` for `proto/` (regenerates `proto/ts/src/ctrl.gen.ts` from the Rust `Ctrl` types and fails when the committed file differs) | `ci-proto.yml` `gen` |
| pre-push | `npm run lint`, `npm run check`, server unit tests in `ui/` (lint also runs at pre-commit; both run `npm ci` in `ui/` first when `ui/node_modules` is missing) | `ci-ui.yml` `check`, `test-server` |
| pre-push | `mise run compat` in `helm/` | `ci-helm.yml` |
| pre-push | `scripts/monorepo/gen-graph.sh --check` and `scripts/monorepo/test-affected.sh` | `ci-monorepo.yml` |

The pre-push hook runs `scripts/monorepo/pre-push.sh`, which checks only the components reported by `affected.sh` (below). `<cargo-args>` mirror each component's `cargo-args` in its `ci-<dir>.yml`: the `-p` list of its crates (`-p enzarb-connector -p enzarb-connector-github -p enzarb-receiver --all-features` for connectors, `-p enzarb-crds -p enzarb-sandbox-operator --all-features` for operator, `-p engine -p enzarb-workflows -p enzarb-flow` for workflows, `--locked -p sandbox-init` for sandbox-container, one `-p` for the rest). Update `scripts/monorepo/pre-push.sh` when those change.

## Affected components
**The dependency graph is generated.** `scripts/monorepo/gen-graph.sh` builds each component's dependency closure from `cargo metadata` (path deps of the root workspace), `package.json` `file:` deps and the hand-written manifest `scripts/monorepo/components.json` (images, non-code inputs such as the engine prompts, graph fixtures, workflow examples and `ui/package-lock.json`, and the CI workflow files). It writes `scripts/monorepo/graph.generated.sh` (`RULES` for `affected.sh`, `DEPS_<dir>` for `deps_of`/`release_paths` in `next-version.sh`, `IMAGE_TABLE` for `image-components.sh`). Never edit that file: change a `Cargo.toml`, `package.json` or the manifest and run `mise run graph:gen`. `gen-graph.sh --check` (CI, `ci-monorepo.yml`) fails when it is stale, and the generator fails on a manifest glob that matches no file. `scripts/monorepo/test-graph.sh` adds a fake Cargo and a fake npm path dependency to a scratch copy and asserts that affected, release paths/versions and the image table follow.

`scripts/monorepo/affected.sh <base>` (or `--files-from -` with paths on stdin) maps `git diff --name-only <base>...HEAD` to components, one per line in dependency order. Its generated rule table is the single source of what each component depends on, including downstream components via path deps (e.g. `proto/**` also affects `workflows`, `sandbox-proxy`, `sandbox-container` and `ui`), and the shared workflow files (`_rust.yml`, `_docker.yml`). `monorepo` is a pseudo-component for the plumbing itself (`scripts/**`, `.mise/tasks/**`, any workflow). Files matching nothing (`docs/**`, root files) affect no component. The pre-push hook and the reviewer prompt use it; CI uses it too (below).

**Changes job.** `.github/workflows/_changes.yml` (`workflow_call`, input `component`, output `run`) checks out with full history and runs `scripts/monorepo/changes.sh <component> <base> <head>`, which feeds `git diff --name-only <base>...<head>` to `affected.sh` and prints `run=true` when the component is listed. The base is the PR base sha, or `github.event.before` on a push to `main`. When there is no usable base (empty or all zeros, a sha missing from the clone after a force push, no merge base) everything runs, so a push to `main` always builds what it should. Each `ci-<dir>.yml` calls it as job `changes` and gives every other job `needs: changes` and `if: needs.changes.outputs.run == 'true'`; the workflow names stay `ci-<dir>` because `release.yml` triggers on them. A `ci-<dir>.yml` or `_changes.yml` edit selects the component(s) that use it. Because the workflows now complete (all jobs skipped) on every push, `release.yml` starts for every component each time; its version check finds nothing releasable for components whose paths did not change and stops.

**Dockerfile COPY blocks.** Every image builds from the repo root (`docker build -f <dir>/Dockerfile .`, root `.dockerignore`). The dependency/source `COPY` lines of each Dockerfile sit between `# BEGIN generated:copy` and `# END generated:copy` and are written by `gen-graph.sh` (`mise run graph:gen`): every workspace member (plus `docker_extra_dirs` from `components.json`) for the Rust images, the `file:` dependency closure of `ui` (`docker_npm_src` narrows a package to the sources the build needs, `proto` to `ts`), minus paths named in `.dockerignore`. `gen-graph.sh --check` fails on a stale or hand-edited block. Adding a Cargo or npm path dependency therefore needs only the manifest edit and `mise run graph:gen`.

`scripts/monorepo/test-affected.sh` (in `ci-monorepo.yml`, with `shellcheck` and `actionlint`) checks the sample diffs, that no `ci-*.yml` has a `paths:` filter and that each gates all its jobs on the `changes` job for its own component, and `changes.sh` including the run-everything fallback.

## Test Postgres
Integration tests that need Postgres (`workflows`, `connectors` receiver, `realtime`, `enzarb-workflows` migrate) use one server per run when `ENZARB_TEST_PG_URL` is set, and one testcontainer per test otherwise. `_rust.yml` takes `postgres: true` (set for workflows, connectors and realtime): it starts one `postgres:18` before the test step and exports the URL. The engine `testing` fixture (`TestDb`) then builds a migrated `enzarb_template` database once (guarded by an advisory lock, rebuilt when the migration checksum changes) and clones it per test.

## Docker images
- **Build once per commit.** One `docker` job in `ci.yml`:
  - **On PRs:** build, and smoke-test if the repo has a smoke script. No push.
  - Every image builds from the repo root (`-f <dir>/Dockerfile .`): the Rust images build the root workspace.
  - **On push to `main`:** build, then push to GHCR with the `main` and `sha-` tags. There are no tag builds (the `ci-*` workflows have no `<dir>/v*` trigger): the version and `latest` tags come from `release.yml` / `rebuild-all`. `release.yml` retags the `sha-` image as `<version>` and `latest` instead of rebuilding it (see Releases).
  - **Remove the separate `image.yml`**, so main isn't built twice.
- **Layer cache:** BuildKit GHA cache with a stable scope per image.
  ```
  --cache-from type=gha,scope=<image>
  --cache-to   type=gha,mode=max,scope=<image>   # only on main pushes
  ```
  - Use `docker buildx build` with `docker/setup-buildx-action`, **plus `crazy-max/ghaction-github-runtime@v3`**. The CLI needs `ACTIONS_RUNTIME_TOKEN`/`ACTIONS_CACHE_URL` for `type=gha`; without them, cache import and export silently do nothing.
  - Pass secrets with `--secret id=<name>_key,env=VAR` (contracts §11), never build-push-action's `secrets:` input.
- **Rust Dockerfiles use [cargo-chef](https://github.com/LukeMathWalker/cargo-chef).**
  - The planner and `cargo chef cook --release` stages build the dependency layer, which the GHA layer cache keeps between runs.
  - The planner copies every workspace member and writes a workspace-wide `recipe.json`; `cook -p <crate>` builds only that crate's dependency graph. The recipe changes only when a `Cargo.toml` or `Cargo.lock` does, so a manifest change in any crate invalidates the cook layer of every Rust image (source-only changes do not).
  - The final `cargo build -p <crate>` only compiles workspace crates, from the sources that crate needs.
  - Per-directory `.dockerignore` files do not apply with the root context; exclusions live in the root `.dockerignore`.
  - BuildKit cache mounts (`--mount=type=cache`) for the cargo registry are fine but do **not** persist on hosted runners. Don't rely on them.
- **Node Dockerfiles:** copy `package.json`/`package-lock.json` and run `npm ci` before copying sources, so the deps layer caches.

## Node repos
- **node_modules:** cache `node_modules` itself with `actions/cache`, keyed on node version + `hashFiles('package-lock.json')`, and skip `npm ci` on a hit (the setup-node `cache: npm` only caches `~/.npm`, so `npm ci` still takes 30-90s). `ui` depends on `file:../proto`, so `.github/actions/ui-install` caches `ui/node_modules` and `proto/node_modules` and rebuilds `proto/ts/dist` on a hit. No GitHub Packages auth is needed.
- **Separate parallel jobs:**
  - `check`: `npm run check` and `npm run lint`.
  - `test-client`: browser component tests on a plain runner. Cache `~/.cache/ms-playwright` keyed on the installed Playwright version, run `npx playwright install chromium` only on a miss, and always run `npx playwright install-deps chromium` (about 15s). A `mcr.microsoft.com/playwright` job container was slower: the image pull costs about 55s per run.
  - `test-server`: node tests (including testcontainers/Docker) on a plain runner. Avoid mounting the Docker socket into a job container.

## Values compatibility
Deploys (notably oldie's HelmChart `valuesContent`, set once by `mise run oldie:bootstrap-helmchart`) keep setting values keys after the chart stops using them, and a key the schema rejects makes the RKE2 helm-install job crash-loop (chart 0.82–0.84 never reached oldie this way).

- **Never remove a values key in the release that stops using it.** Keep it in `values.schema.json` as an accepted, ignored key whose `description` starts with `DEPRECATED:` (see `operator.docker.resources` / `operator.docker.image`). Remove it only after every deploy no longer sets it.
- **`mise run compat`** (in `helm/`, part of `mise run test`, run by `ci-helm` with full history so tags are present) lints and templates the current chart with the latest `helm/v*` tag's `values.yaml` and `values-oldie.yaml` (read with `git show`), each passed alone as `-f`, as the HelmChart does. A key removed in a PR but still set by the previous release fails. With no `helm/v*` tag it prints a skip notice and passes. It checks the repo-tracked files only, not the live HelmChart `valuesContent`. **Limitation:** the schema root has `additionalProperties: true`, so a removed *top-level* key is silently accepted and not caught; only keys inside the strict sub-objects (`ui`, `operator`, `updater`, `secrets.*`, …) are. The 0.82–0.84 keys (`operator.docker.*`) were nested, so they are covered.
- **Visibility.** The updater serves `GET /status` (`installed`, `desired`, `latest`, `stuck`) and logs a failing upgrade; the UI About page shows it.

## Reporting
A PR that changes CI states before/after wall-clock times for the PR run and the main run, taken from the Actions UI or `gh run view --json jobs`.

## Releases
`.github/workflows/release.yml` releases every image component (`ui`, `workflows`, `connectors`, `sandbox-proxy`, `operator`, `sandbox-container`, `realtime`, `updater`) and `helm`, always on and with only the built-in `GITHUB_TOKEN` (`contents: write`, `packages: write`). It runs on `workflow_run` of their `ci-<dir>` workflows (listed by name; add new ones there) and on `workflow_dispatch` with a `component` input for manual reruns. Everything happens in that one run: `GITHUB_TOKEN` tags and pushes trigger no workflow, so nothing depends on a tag build, a `ci-helm` run on the bot's commit or another release run.

- **Versions come from commits.** After `ci-<dir>` succeeds on a push to `main`, `.github/scripts/next-version.sh` reads the Conventional Commits since the last `<dir>/vX.Y.Z` tag that touch `<dir>` or one of its path-dependency dirs (`git log --ancestry-path`, so history imported before the monorepo does not count): `feat` bumps minor, `fix`/`perf`/`revert` bump patch, `type!:` or a `BREAKING CHANGE:` footer bumps major (while 0.x, breaking and `feat` both bump minor). History with only `chore`/`docs`/`ci`/`test`/`refactor` releases nothing. A component's first release is `0.1.0`. A `feat` in `proto/` therefore releases `ui`, `workflows`, `sandbox-proxy` and `sandbox-container`.
- **Image, tag and release** (`_release-image.yml`, shared with rebuild-all). Retag first (`.github/scripts/retag-or-build.sh`): if the CI image `ghcr.io/enzarb/<image>:sha-<short>` exists and its `org.opencontainers.image.revision` label is exactly the released sha, `docker buildx imagetools create` adds `<version>` and `latest` to it with no rebuild, then checks that both resolve to the CI digest (this saves `sandbox-container` its long build). Otherwise (no CI image because the changes job skipped it or the build failed, a label mismatch, any registry error, or `rebuild-all`) it builds `ghcr.io/enzarb/<image>` at the released sha (names, contexts, smoke test and timeouts from `.github/scripts/image-components.sh`, matching `_docker.yml`) and pushes `<version>`, `latest` and `sha-<short>`. The log and step summary say which path ran: `release path: retag of sha-abc1234 (sha256:...)` or `release path: rebuild (<reason>)`. A retagged image carries the CI OCI labels, so its `org.opencontainers.image.version` is not the release version (a rebuilt one has it). Either way the job then creates the annotated tag `<dir>/vX.Y.Z` and a GitHub Release (notes start at the component's previous tag; `--latest=false`, so the repo's latest release is always the chart). A tag therefore always means its image exists. Releases are serialized per component (workflow concurrency `release-ci-<dir>`).
- **Settle** (`release.yml`, `.github/scripts/wait-siblings.sh`). Between `image` and `chart`, each release run waits until every *other* image component that `next-version.sh` says is releasable from the same sha has published its `<dir>/vX.Y.Z` tag, or is known not to: its `ci-<dir>` run for that sha failed, none appeared within 5 minutes (path-filtered), or its CI succeeded but no release run for the sha is still active after 5 minutes (image build failed). It polls every 30s and gives up after 40 minutes with a `::warning::`, publishing a partial chart that a later release run or `sync-components` completes. The first run to settle publishes the chart, which pins every tag that exists; the other runs for the sha find nothing to do. A commit that releases several components (say `workflows` and `ui`) therefore normally yields one chart version carrying all of them, so a consumer never rolls out ahead of the component it depends on.
- **Chart** (`_release-chart.yml`, shared with rebuild-all and sync-components; one at a time, concurrency `chart`). `helm/scripts/sync-components.sh` (with `SKIP_IMAGE_CHECK=1`) reconciles *every* component in `helm/scripts/components.yaml` to its highest `<dir>/v*` tag in `helm/enzarb/values.yaml` (`operator.sandboxImage` for the sandbox image; the Project CRD from `operator/deploy/crds/project.yaml` at the operator tag, read with `git show`). It refreshes the goldens, runs `mise run test` and commits `feat(deps): bump ...` / `fix(deps): ...` to `main` (one rebase-and-retry if `main` moved). The next chart version comes from `next-version.sh` for `helm` (that commit plus any other `helm/` commits since the last `helm/v*` tag; a minor/major component bump is a `feat`, a patch bump a `fix`). The job packages `--version X.Y.Z`, pushes `oci://ghcr.io/enzarb/charts/enzarb`, then tags `helm/vX.Y.Z` and creates the release marked latest (which the updater deploys). `Chart.yaml` keeps `0.0.0` placeholders. A newer pending chart run replaces an older pending one, which is safe because every run reconciles all tags.
- **Human chart changes.** A push under `helm/` runs `ci-helm`; on success `release.yml` runs only the chart job, which releases the chart from those commits.
- **Safety net.** `sync-components.yml` (6-hourly schedule and manual dispatch) runs the same chart job against `main`, publishing a chart in the same run if it pinned anything or found releasable `helm/` commits. `publish-helm.yml` is manual only: it republishes an existing `helm/vX.Y.Z` tag to GHCR and makes its release latest.
- **Branch protection:** the chart job pushes to `main` with `GITHUB_TOKEN`. A protection rule or ruleset that blocks that push (required PRs or status checks without a bypass for GitHub Actions) breaks the chart step.
- `proto` and `kubernetes` are not released. `proto` is consumed in-tree (Rust path deps, `ui` via `file:../proto`), so `@enzarb/proto` is no longer published to GitHub Packages. `kubernetes` pushes its gvisor installer image on `main` (`main` and `sha-` tags) and, with version and `latest` tags, via `rebuild-all`'s installer job.
- Local check: `.github/scripts/next-version.sh <dir> [<sha>]` prints a component's last tag and next version for the real history. Its path list (`release_paths`) is generated, see "Affected components".
- Version detection reads Conventional Commit subjects. Squash-merge titles count; git's default `Revert "..."` subject does not match `revert:`, so write reverts as `revert: ...` if they should release.

## Rebuild everything
`.github/workflows/rebuild-all.yml` (manual `workflow_dispatch` on `main`, inputs `bump: patch|minor` and `dry_run`) force-releases every component regardless of commit history. Like `release.yml` it uses only `GITHUB_TOKEN` and the same `_release-image.yml` / `_release-chart.yml`.

- **plan** computes the next patch (or minor) after each component's highest `<dir>/v*` tag; `kubernetes` (gvisor installer image) starts at `kubernetes/v0.1.0`.
- **images** (matrix) builds each image on the current `main` sha with the same names, contexts and `sandbox-container` smoke test as `_docker.yml`, pushes `<version>`, `latest` and `sha-<short>`, then creates the annotated `<dir>/v<version>` tag and a `--latest=false` release. **installer** does the same for `ghcr.io/enzarb/enzarb-gvisor-installer` (amd64 smoke test, amd64+arm64 push).
- **chart** (`_release-chart.yml` with the forced `bump`) runs `helm/scripts/sync-components.sh` with `SKIP_IMAGE_CHECK=1` against the new tags (values + CRD), refreshes goldens, runs `mise run test`, commits `fix(deps): rebuild all components` to `main` (one rebase-and-retry if `main` moved), tags `helm/v<next>`, pushes `oci://ghcr.io/enzarb/charts/enzarb` and creates the release marked latest (which the updater deploys).
- `dry_run` builds everything (`--load`), syncs and tests the chart, and packages it, but pushes, tags, commits and releases nothing. One run at a time (`concurrency: rebuild-all`). If `main` is branch-protected against `GITHUB_TOKEN` pushes, the chart commit step fails after the images are released; re-run `sync-components` instead.
- It always rebuilds (`_release-image.yml` runs with `retag: false`), never reusing the CI `sha-` image.

## Testing the release path
`scripts/monorepo/test-retag-or-build.sh` tests the retag decision and the digest check with a stub `docker` (retag, no CI image, revision mismatch, `retag=false`, dry run, digest mismatch, `verify` for single-manifest and index sources, and a negative test where the script without `--prefer-index=false` must fail). The `scripts` job of `ci-monorepo.yml` runs it. Both paths only run for real in Actions: dispatch `release.yml` for a component whose latest `main` sha has a CI image (retag; the digest of `:<version>` equals `:sha-<short>`), and for a sha without one, or `rebuild-all` with `dry_run`, to see the rebuild path. `dry_run` with a CI image only logs `would retag`.
