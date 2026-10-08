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
- **Toolchain:** from `rust-toolchain.toml` via `actions-rust-lang/setup-rust-toolchain`. Turn off its built-in cache (`cache: false`); we use rust-cache.
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
  - Use `docker buildx build` with `docker/setup-buildx-action`.
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
