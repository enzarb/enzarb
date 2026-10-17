## Summary

Make the root `mise.toml` the only place tool versions are pinned. Component `mise.toml` files keep only tasks. A new script, `scripts/monorepo/check-tool-pins.sh`, runs in CI and lefthook. It fails when a Dockerfile `ARG` tool version differs from the root pin, and when any component `mise.toml` has a `[tools]` section.

## Approach

**Corrections to the triage.** `operator/` has no `mise.toml`. The 10 files are `connectors`, `workflows`, `realtime`, `sandbox-proxy`, `updater`, `proto`, `ui`, `sandbox-container`, `helm` and `kubernetes`. Nothing in the repo (`rust-toolchain.toml` is checked in) forces a `rust` pin through mise: each Rust component pins its channel in `rust-toolchain.toml` (e.g. `1.99.0`).

**Pins.** I will use concrete versions everywhere and avoid `latest`. I could not run mise or reach registries in this planning step. The implementer should take current values from `mise ls` or `mise latest <tool>` and record them in the PR.
- `node = "24.21.0"`. This equals `ARG NODE_VERSION` in `sandbox-container/Dockerfile`, which has run in production, and it replaces `lts` and `24`.
- `helm = 4.3.0`, `kubeconform = 0.8.0`, `shellcheck = 0.11.0`, `kubectl = 1.34.0` and `yq = 4.54.1`. These are the values components already pinned. Where root said `latest` the component pin wins.
- `gh = 2.76.2`, equal to `ARG GH_VERSION`.
- `python = 3.13`.
- `cargo-nextest`: pin it as `aqua:nextest-rs/nextest/cargo-nextest`.
- `claude`, `kind` and `npm`: the currently resolved versions.
- `rust`: `rust-toolchain.toml` already pins each component's compiler, so a root `rust` pin only provides rustup. Pin it to the same version as the toolchain files (`1.99.0`) and add the check described below that this agrees with every `rust-toolchain.toml`. This catches drift between the files.

**Ansible (needs a decision).** `docs/dev.md:47` says the root `mise.toml` must stay free of `pipx:` tools. mise installs every `[tools]` entry before any task runs, and the sandbox runs `mise install` at the root. Putting `pipx:ansible-core` and `pipx:ansible-lint` in the root `[tools]` would slow every task and sandbox start-up. Chosen approach: a root-level env file `mise.ansible.toml`, activated with `MISE_ENV=ansible`. It is still at the root, and it holds only the two ansible pins. `ci-kubernetes.yml` (ansible job) and `.mise/tasks/oldie/{host,reset}` (which currently re-exec under `kubernetes/mise.toml`) set `MISE_ENV=ansible`. The root file keeps the check's "all pins are in root" invariant. Rejected alternative: put ansible in the root `[tools]` and rewrite the `docs/dev.md` rule (simpler, but every sandbox pays the build). See open question 1.

**CI node.** Replace the `node-version: lts/*` entries in `ci-ui.yml` (four places) and `ci-proto.yml` with `node-version-file` pointing at a file the root pin feeds, or switch those jobs to `jdx/mise-action`. `setup-node` cannot read `mise.toml` natively. I prefer `jdx/mise-action` with `cache: true`, which is already the tools convention in `docs/ci.md`. The `_rust.yml` `inputs.node` and the `node: '24'` in `ci-sandbox-container.yml` must also come from the root pin. The simplest way is to have them read it via `mise current node`.

**ci-kubernetes.** Drop the `MISE_CEILING_PATHS` and `working_directory: kubernetes` isolation. The job then uses the root pins, with `MISE_ENV=ansible` only on the ansible job. Keep the `mise current helm` vs `HELM_VERSION` check (`versions.env` still holds `HELM_VERSION` for the installer image).

**The check script (`scripts/monorepo/check-tool-pins.sh`).** It is plain bash, like `check-cargo-deps.sh`, and it uses an explicit mapping table at the top, one row per `Dockerfile:ARG → mise tool` (rows are added by hand):
- `sandbox-container/Dockerfile`: `NODE_VERSION` → node, `GH_VERSION` → gh.
- `PLAYWRIGHT_VERSION` → delegate to the existing `sandbox-container/scripts/check-playwright-version.sh`, which compares it against `ui/package-lock.json`. Playwright is an npm package and is not in mise.

Tools that appear only as Dockerfile `ARG`s (docker, buildx, compose, sccache, mise, claude-agent-acp, gVisor) are listed as explicitly unmapped, so a reader can see what is deliberately not covered. A new `ARG *_VERSION` that is neither mapped nor listed unmapped fails the script, so the table cannot silently go stale.

The script also checks that:
1. No `*/mise.toml` contains a `[tools]` section (acceptance criterion 1).
2. No root pin is `latest`.
3. Root `rust` equals each `rust-toolchain.toml` channel.
4. `kubernetes/hack/versions.env` `HELM_VERSION` equals the root `helm`, which replaces the inline check in `ci-kubernetes.yml`.

Version parsing uses `grep`/`sed` on the TOML (no new dependency). Comparison is exact, after stripping a leading `v`.

## Changes

- `mise.toml`: concrete pins as above; remove `latest`.
- `mise.ansible.toml` (new): `pipx:ansible-core=2.21.5`, `pipx:ansible-lint=26.9.0`.
- `{connectors,workflows,realtime,sandbox-proxy,updater,proto,ui,sandbox-container,helm,kubernetes}/mise.toml`: remove `[tools]`, keep the tasks and comments. If a file ends up empty, keep it with a comment only if mise tasks need the directory context; otherwise delete it.
- `.mise/tasks/oldie/host`, `.mise/tasks/oldie/reset`: set `MISE_ENV=ansible` instead of re-running under the `kubernetes` toolset.
- `scripts/monorepo/check-tool-pins.sh` (new) and `scripts/monorepo/test-tool-pins.sh` (new).
- `.mise/tasks/monorepo/tool-pins` (optional wrapper if there is a `mise run` convention for it; otherwise skip).
- `lefthook.yml`: add a `tool-pins` command to pre-commit. Glob: `{mise.toml,mise.*.toml,*/mise.toml,**/Dockerfile,**/rust-toolchain.toml,kubernetes/hack/versions.env}`.
- `.github/workflows/ci-monorepo.yml`: add a `tool-pins` step to the `scripts` job. Add the new paths to that workflow's `paths:` filters (root `mise.toml`, `*/mise.toml`, `**/Dockerfile`). Shellcheck already covers `scripts/monorepo/*.sh`.
- `.github/workflows/ci-ui.yml`, `ci-proto.yml`, `ci-sandbox-container.yml`, `_rust.yml`, `ci-kubernetes.yml`: take node and other tool versions from the root pin as described above.
- `scripts/monorepo/affected.sh` and `test-affected.sh`: update only if the new filters change the rule table. They must stay equal to the `paths:` filters.
- `docs/ci.md`: the "Tools" bullet (root `mise.toml` is the only pin site; `latest` is not allowed), the hooks table (new row) and a new description of the pin check.
- `docs/dev.md`: line 47 rewritten for `mise.ansible.toml` and `MISE_ENV=ansible`. Add a short "Adding or bumping a tool" note: bump the root pin and the matching Dockerfile `ARG` in the same PR.
- `sandbox-container/src/prepare.rs` and its docs: no change expected. It runs `mise install` for the root and for every directory with a `mise.toml`, and that still works. Verify that it still behaves correctly when a component `mise.toml` has no `[tools]`.

## Tests

- `test-tool-pins.sh`, in the style of `test-affected.sh` and `test-mechanical.sh`. It runs the check against temp-directory fixtures: the passing case, a root `node` pin bumped without the `ARG` (must fail), a `[tools]` section in a component file (must fail), a `latest` pin (must fail), and an unmapped new `ARG *_VERSION` (must fail). It runs in `ci-monorepo.yml` next to the other script tests.
- Run the real check on the repo, then deliberately change the root `node` pin locally and confirm it fails.
- Acceptance: `grep -n '^\[tools\]' */mise.toml` prints nothing.
- Component checks, from inside each directory: Rust components `cargo clippy --all-targets && cargo test`; `proto`, `ui` `npm test` (and `npm run check && npm run lint` for `ui`); `helm` `mise run test`; `kubernetes` `kubectl kustomize` and `./hack/validate.sh`; `sandbox-container` `cargo test` (and the Docker build if the Dockerfile changes). `shellcheck` on the new scripts. `actionlint` on the changed workflows.
- Confirm that `mise run` tasks in each component still find their tools from the root, and that CI is green on the PR.

## Risks and open questions

1. **Ansible placement.** `mise.ansible.toml` plus `MISE_ENV` versus root `[tools]`, as above. The `MISE_ENV` approach needs checking in CI, because mise-action's cache key must include the env file.
2. **Exact "current" versions** for `claude`, `kind`, `npm` and `rust`. They must be read from a real `mise ls`. A wrong guess could break CI.
3. **Root toolset size.** All component tools in one file means every task installs more. In particular `mise install` in the sandbox (`prepare.rs`) now also installs `python`, `yq`, `cargo-nextest` and `kubectl`. Install time and sandbox image impact should be reviewed.
4. **Rust pin.** Pinning root `rust` while each component keeps its own `rust-toolchain.toml` is redundant by design. The agreement check is what keeps them honest. Alternative: leave `rust` unpinned and document it as an exception, which contradicts "pin every tool".
5. **Node bump.** `lts` → `24.21.0` can differ from the version CI used before. `ui` and `proto` need a local run on the new version.
6. `gh` moves from `latest` to `2.76.2` for local use, which is older than whatever `latest` resolves to now. If that is unwanted, bump both the root pin and `ARG GH_VERSION`.

## Out of scope

- Mapping Docker-only tools (docker, buildx, compose, sccache, mise, gVisor) to root pins.
- Automated version-bump tooling (Renovate/Dependabot) for the pins.
- Adding a mise lockfile.
- Changing the Rust toolchain versions or the Playwright and `ui` dependency versions.

Reply with feedback to revise this plan, or comment /approve to start the implementation.
