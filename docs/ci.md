# CI conventions

Every enzarb repo's GitHub Actions setup follows these rules. The goals are fast feedback on PRs, building each artifact once, and caching that persists between runs.

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
  - `fmt`: `cargo fmt --check` plus the "no `[patch]` in Cargo.lock" guard. No cache, no deps fetch.
  - `clippy`: `cargo clippy --all-targets [--all-features] -- -D warnings`.
  - `test`: `cargo nextest run` (installed with `taiki-e/install-action@nextest`) plus `cargo test --doc` if the crate has doctests. Integration tests that need Docker run here.
- **Cache:** `Swatinem/rust-cache@v2` in clippy and test.
  - Use a distinct `key` per job, because clippy and test build different artifacts.
  - Set `save-if: ${{ github.ref == 'refs/heads/main' }}`. PRs read the main cache but don't write, so cache churn stays low.
- **Private enzarb git deps:** the ssh-agent step from contracts §11 runs in each job that resolves dependencies (clippy, test, docker).

## Docker images
- **Build once per commit.** One `docker` job in `ci.yml`:
  - **On PRs:** build, and smoke-test if the repo has a smoke script. No push.
  - **On push to `main` and `v*` tags:** build, then push to GHCR with the `docker/metadata-action` tags from contracts §13.
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
  - Private git deps are fetched in the cook stage, using the build-secret pattern.
  - BuildKit cache mounts (`--mount=type=cache`) for the cargo registry are fine but do **not** persist on hosted runners. Don't rely on them.
- **Node Dockerfiles:** copy `package.json`/`package-lock.json` and run `npm ci` before copying sources, so the deps layer caches.

## Node repos
- **node_modules:** cache `node_modules` itself with `actions/cache`, keyed on node version + `hashFiles('package-lock.json')`, and skip `npm ci` on a hit (the setup-node `cache: npm` only caches `~/.npm`, so `npm ci` still takes 30-90s). The repo's `.npmrc` reads `NODE_AUTH_TOKEN` for GitHub Packages. Share this as a local composite action.
- **Separate parallel jobs:**
  - `check`: `npm run check` and `npm run lint`.
  - `test-client`: browser component tests on a plain runner. Cache `~/.cache/ms-playwright` keyed on the installed Playwright version, run `npx playwright install chromium` only on a miss, and always run `npx playwright install-deps chromium` (about 15s). A `mcr.microsoft.com/playwright` job container was slower: the image pull costs about 55s per run.
  - `test-server`: node tests (including testcontainers/Docker) on a plain runner. Avoid mounting the Docker socket into a job container.

## Reporting
A PR that changes CI states before/after wall-clock times for the PR run and the main run, taken from the Actions UI or `gh run view --json jobs`.

## Releases
Every image repo (`enzarb-ui`, `-workflows`, `-connectors`, `-sandbox-proxy`, `-sandbox-operator`, `-sandbox-container`, `-realtime`) and `enzarb-helm` runs the same `.github/workflows/release.yml`. The canonical copy is `docs/ci/release.yml` here; edit it and run `mise run ci:sync-release` (`--check` reports drift).

- **Versions come from commits.** After `ci` succeeds on `main`, the workflow reads the Conventional Commits since the last `v*` tag: `feat` bumps minor, `fix`/`perf`/`revert` bump patch, `type!:` or a `BREAKING CHANGE:` footer bumps major (while 0.x, breaking and `feat` both bump minor). History with only `chore`/`docs`/`ci`/`test`/`refactor` releases nothing. The first release of a repo is `v0.1.0`. Use Conventional Commit subjects (squash-merge titles included) and the rest follows.
- **Tag and release.** The workflow creates the annotated tag and a GitHub Release with generated notes, using `ENZARB_RELEASE_TOKEN` (not `GITHUB_TOKEN`: tags pushed with that token do not trigger workflows). The tag push runs `ci` again, whose `docker` job pushes `ghcr.io/enzarb/<image>:<version>` (and `latest`).
- **Chart sync.** When the tag's `ci` run succeeds, the component dispatches `component-released` to `enzarb-helm`. Its `sync-components` workflow (also on a 6-hourly schedule as a safety net) reconciles *every* component in `scripts/components.yaml` to its latest GitHub release, but only advances one whose image tag exists in GHCR. It rewrites the tags in `enzarb/values.yaml` (and the Project CRD for the operator), refreshes the golden files, runs the chart checks and commits `feat(deps): bump ...` / `fix(deps): ...` to `main`. Because the payload is only a trigger, concurrent or lost dispatches cannot drop a bump.
- **Chart release.** That commit goes through the same release workflow, so the chart gets its own version (a minor/major component bump is a `feat`, a patch bump a `fix`). `publish.yml` packages the chart at the tag's version and pushes `oci://ghcr.io/enzarb/charts/enzarb`. `Chart.yaml` keeps `0.0.0` placeholders.
- **Setup per repo:** secret `ENZARB_RELEASE_TOKEN` and then the repo variable `RELEASE_ENABLED=true` (the workflow is dormant until both exist), a fine-grained PAT on the `enzarb` org with Contents read/write and Actions read on all enzarb repos (Actions read lets the chart sync confirm that a release tag's build succeeded, which is how it knows the image was pushed; GHCR itself is not accessed with this token), plus Workflows write for the repo that first adds `release.yml`. Org secrets are not visible to private repos on the Free plan, so set it as a repo-level secret on each of the seven repos (like the deploy keys in contracts.md section 11).
- `enzarb-proto`, `enzarb-kubernetes` and the root repo are not released this way: proto is consumed as a git dependency on `main` and published by its own `publish.yml`.
- Version detection reads Conventional Commit subjects. Squash-merge titles count; git's default `Revert "..."` subject does not match `revert:`, so write reverts as `revert: ...` if they should release.
