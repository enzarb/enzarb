# CI conventions

The monorepo's GitHub Actions setup (root `.github/`) follows these rules. Each component has its own `ci-<dir>.yml`, filtered with `paths:` to the component dir, its path-dependency dirs and its own workflow files, so a change only runs the CI of what it can affect. Rust components share the reusable `_rust.yml`; image builds share `_docker.yml`. The goals are fast feedback on PRs, building each artifact once, and caching that persists between runs.

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
- **Tools:** come from `jdx/mise-action` with `cache: true`, reading the repo's `mise.toml`. No `curl | bash` installers.

## Rust repos
- **Toolchain:** pin an exact version in `rust-toolchain.toml` (e.g. `channel = "1.99.0"`), never `stable`. Runners resolve `stable` to different versions, which changes rust-cache's environment hash so every run misses. Install it via `actions-rust-lang/setup-rust-toolchain` with its built-in cache off (`cache: false`); we use rust-cache.
- **Separate, parallel jobs:**
  - `fmt`: `cargo fmt --check` plus a guard that no `[patch]` and no git dependency on an enzarb repo remains (enzarb crates are path deps, `../<dir>`). No cache, no deps fetch.
  - `clippy`: `cargo clippy --all-targets [--all-features] -- -D warnings`.
  - `test`: `cargo nextest run` (installed with `taiki-e/install-action@nextest`) plus `cargo test --doc` if the crate has doctests. Integration tests that need Docker run here.
- **Cache:** `Swatinem/rust-cache@v2` in clippy and test.
  - `workspaces: <dir> -> target` and a distinct `key` per component and job (`<dir>-clippy`, `<dir>-test`), because clippy and test build different artifacts.
  - Set `save-if: ${{ github.ref == 'refs/heads/main' }}`. PRs read the main cache but don't write, so cache churn stays low.
- **Toolchain file per component:** `setup-rust-toolchain` gets `rust-src-dir: <dir>`; jobs run with `working-directory: <dir>`.

## Docker images
- **Build once per commit.** One `docker` job in `ci.yml`:
  - **On PRs:** build, and smoke-test if the repo has a smoke script. No push.
  - **On push to `main`:** build, then push to GHCR with the `main` and `sha-` tags. There are no tag builds (the `ci-*` workflows have no `<dir>/v*` trigger): the version and `latest` tags come from `release.yml` / `rebuild-all`.
  - Components with path deps (`sandbox-proxy`, `sandbox-container`, `workflows`, `ui`) build from the repo root (`-f <dir>/Dockerfile .`); the others use their dir as context.
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
  - The final `cargo build` only compiles workspace crates.
  - BuildKit cache mounts (`--mount=type=cache`) for the cargo registry are fine but do **not** persist on hosted runners. Don't rely on them.
- **Node Dockerfiles:** copy `package.json`/`package-lock.json` and run `npm ci` before copying sources, so the deps layer caches.

## Node repos
- **node_modules:** cache `node_modules` itself with `actions/cache`, keyed on node version + `hashFiles('package-lock.json')`, and skip `npm ci` on a hit (the setup-node `cache: npm` only caches `~/.npm`, so `npm ci` still takes 30-90s). `ui` depends on `file:../proto`, so `.github/actions/ui-install` caches `ui/node_modules` and `proto/node_modules` and rebuilds `proto/ts/dist` on a hit. No GitHub Packages auth is needed.
- **Separate parallel jobs:**
  - `check`: `npm run check` and `npm run lint`.
  - `test-client`: browser component tests on a plain runner. Cache `~/.cache/ms-playwright` keyed on the installed Playwright version, run `npx playwright install chromium` only on a miss, and always run `npx playwright install-deps chromium` (about 15s). A `mcr.microsoft.com/playwright` job container was slower: the image pull costs about 55s per run.
  - `test-server`: node tests (including testcontainers/Docker) on a plain runner. Avoid mounting the Docker socket into a job container.

## Reporting
A PR that changes CI states before/after wall-clock times for the PR run and the main run, taken from the Actions UI or `gh run view --json jobs`.

## Releases
`.github/workflows/release.yml` releases every image component (`ui`, `workflows`, `connectors`, `sandbox-proxy`, `operator`, `sandbox-container`, `realtime`, `updater`) and `helm`, always on and with only the built-in `GITHUB_TOKEN` (`contents: write`, `packages: write`). It runs on `workflow_run` of their `ci-<dir>` workflows (listed by name; add new ones there) and on `workflow_dispatch` with a `component` input for manual reruns. Everything happens in that one run: `GITHUB_TOKEN` tags and pushes trigger no workflow, so nothing depends on a tag build, a `ci-helm` run on the bot's commit or another release run.

- **Versions come from commits.** After `ci-<dir>` succeeds on a push to `main`, `.github/scripts/next-version.sh` reads the Conventional Commits since the last `<dir>/vX.Y.Z` tag that touch `<dir>` or one of its path-dependency dirs (`git log --ancestry-path`, so history imported before the monorepo does not count): `feat` bumps minor, `fix`/`perf`/`revert` bump patch, `type!:` or a `BREAKING CHANGE:` footer bumps major (while 0.x, breaking and `feat` both bump minor). History with only `chore`/`docs`/`ci`/`test`/`refactor` releases nothing. A component's first release is `0.1.0`. A `feat` in `proto/` therefore releases `ui`, `workflows`, `sandbox-proxy` and `sandbox-container`.
- **Image, tag and release** (`_release-image.yml`, shared with rebuild-all). Builds `ghcr.io/enzarb/<image>` at the released sha (names, contexts, smoke test and timeouts from `.github/scripts/image-components.sh`, matching `_docker.yml`), pushes `<version>`, `latest` and `sha-<short>`, then creates the annotated tag `<dir>/vX.Y.Z` and a GitHub Release (notes start at the component's previous tag; `--latest=false`, so the repo's latest release is always the chart). A tag therefore always means its image exists. Releases are serialized per component (workflow concurrency `release-ci-<dir>`).
- **Chart** (`_release-chart.yml`, shared with rebuild-all and sync-components; one at a time, concurrency `chart`). `helm/scripts/sync-components.sh` (with `SKIP_IMAGE_CHECK=1`) reconciles *every* component in `helm/scripts/components.yaml` to its highest `<dir>/v*` tag in `helm/enzarb/values.yaml` (`operator.sandboxImage` for the sandbox image; the Project CRD from `operator/deploy/crds/project.yaml` at the operator tag, read with `git show`). It refreshes the goldens, runs `mise run test` and commits `feat(deps): bump ...` / `fix(deps): ...` to `main` (one rebase-and-retry if `main` moved). The next chart version comes from `next-version.sh` for `helm` (that commit plus any other `helm/` commits since the last `helm/v*` tag; a minor/major component bump is a `feat`, a patch bump a `fix`). The job packages `--version X.Y.Z`, pushes `oci://ghcr.io/enzarb/charts/enzarb`, then tags `helm/vX.Y.Z` and creates the release marked latest (which the updater deploys). `Chart.yaml` keeps `0.0.0` placeholders. A newer pending chart run replaces an older pending one, which is safe because every run reconciles all tags.
- **Human chart changes.** A push under `helm/` runs `ci-helm`; on success `release.yml` runs only the chart job, which releases the chart from those commits.
- **Safety net.** `sync-components.yml` (6-hourly schedule and manual dispatch) runs the same chart job against `main`, publishing a chart in the same run if it pinned anything or found releasable `helm/` commits. `publish-helm.yml` is manual only: it republishes an existing `helm/vX.Y.Z` tag to GHCR and makes its release latest.
- **Branch protection:** the chart job pushes to `main` with `GITHUB_TOKEN`. A protection rule or ruleset that blocks that push (required PRs or status checks without a bypass for GitHub Actions) breaks the chart step.
- `proto` and `kubernetes` are not released. `proto` is consumed in-tree (Rust path deps, `ui` via `file:../proto`), so `@enzarb/proto` is no longer published to GitHub Packages. `kubernetes` pushes its gvisor installer image on `main` (`main` and `sha-` tags) and, with version and `latest` tags, via `rebuild-all`'s installer job.
- Local check: `scripts/monorepo/test-release-version.sh` prints each component's last tag and its next version for synthetic `feat`/`fix` commits, and which `ci-*` workflows sample paths trigger.
- Version detection reads Conventional Commit subjects. Squash-merge titles count; git's default `Revert "..."` subject does not match `revert:`, so write reverts as `revert: ...` if they should release.

## Rebuild everything
`.github/workflows/rebuild-all.yml` (manual `workflow_dispatch` on `main`, inputs `bump: patch|minor` and `dry_run`) force-releases every component regardless of commit history. Like `release.yml` it uses only `GITHUB_TOKEN` and the same `_release-image.yml` / `_release-chart.yml`.

- **plan** computes the next patch (or minor) after each component's highest `<dir>/v*` tag; `kubernetes` (gvisor installer image) starts at `kubernetes/v0.1.0`.
- **images** (matrix) builds each image on the current `main` sha with the same names, contexts and `sandbox-container` smoke test as `_docker.yml`, pushes `<version>`, `latest` and `sha-<short>`, then creates the annotated `<dir>/v<version>` tag and a `--latest=false` release. **installer** does the same for `ghcr.io/enzarb/enzarb-gvisor-installer` (amd64 smoke test, amd64+arm64 push).
- **chart** (`_release-chart.yml` with the forced `bump`) runs `helm/scripts/sync-components.sh` with `SKIP_IMAGE_CHECK=1` against the new tags (values + CRD), refreshes goldens, runs `mise run test`, commits `fix(deps): rebuild all components` to `main` (one rebase-and-retry if `main` moved), tags `helm/v<next>`, pushes `oci://ghcr.io/enzarb/charts/enzarb` and creates the release marked latest (which the updater deploys).
- `dry_run` builds everything (`--load`), syncs and tests the chart, and packages it, but pushes, tags, commits and releases nothing. One run at a time (`concurrency: rebuild-all`). If `main` is branch-protected against `GITHUB_TOKEN` pushes, the chart commit step fails after the images are released; re-run `sync-components` instead.
