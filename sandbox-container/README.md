# enzarb-sandbox-container

The sandbox image (`ghcr.io/enzarb/enzarb-sandbox`) and its pid-1 `sandbox-init`.

```sh
docker buildx build --secret id=proto_key,src=$HOME/.ssh/id_ed25519 --load -t enzarb-sandbox:dev .
scripts/smoke.sh enzarb-sandbox:dev
```

## What is in the image

Only system components:

- `sandbox-init` (entrypoint)
- node 24, `@agentclientprotocol/claude-agent-acp` (pinned) and the glibc `claude` binary, installed directly (not through mise). `claude-agent-acp` and `claude` are symlinks in `/usr/local/bin`.
- `mise`
- `build-essential`, `pkg-config`, `libssl-dev`
- `git`, `curl`, `ca-certificates`, `openssh-client`

There is no rustup/Rust, npm, ripgrep, jq or other CLI tool, and no `HEALTHCHECK` (Kubernetes probes `/healthz`). `scripts/smoke.sh` asserts both what is present and what is absent.

## Toolchains come from mise

All user and project toolchains are installed by mise onto the `/workspace` PVC, so they persist per sandbox:

| Variable | Value |
|---|---|
| `MISE_DATA_DIR` | `/workspace/.toolchains/mise` |
| `MISE_CACHE_DIR` | `/workspace/.toolchains/cache` |
| `CARGO_HOME` / `RUSTUP_HOME` | `/workspace/.toolchains/cargo` / `.../rustup` |
| `MISE_YES=1` | never prompt (there is no tty) |
| `MISE_TRUSTED_CONFIG_PATHS=/workspace` | repo `mise.toml` files under `/workspace` are trusted (without it mise refuses to read them) |
| `MISE_IDIOMATIC_VERSION_FILE_ENABLE_TOOLS=rust` | read `rust-toolchain.toml` (idiomatic version files are off by default) |
| `PATH` | `/usr/local/bin`, then `/workspace/.toolchains/mise/shims`, then the system dirs |

`sandbox-init` passes its environment on to the ACP agent and to PTY shells, so agents see the shims and run project tools (`cargo`, `node`, ...) once those are installed.

### What a project repo needs

- A `mise.toml` with its tools, e.g. `[tools]` `rust = "1.99.0"`. A `rust-toolchain.toml` (channel, components, targets) alone is also picked up. Run `mise install` in the repo.
- node tools via mise get their own node; the system `node` only runs the agent adapter.

### Rust

mise installs Rust through rustup, whose installer needs `curl` (in the image), so `mise install` in a repo with a `mise.toml` or `rust-toolchain.toml` works directly.

Afterwards `cargo`, `rustc`, `clippy` and `rustfmt` resolve through the shims.

## Permission policy

The default command allowlist in `src/policy.rs` names programs, not paths. Entries for tools that are not baked in (`cargo`, `npm`, `rg`, `jq`, `tree`) are harmless: they match only once mise has installed the tool (or never). `rg`, `jq` and `tree` can be added per project with mise.

`python3` is allowed like `bash`/`sh`: only `python3 [-u -B -E -s -S -I -O -q] <script in the worktree> [args]`; `-c`, `-m`, stdin and other options are refused. Python is not baked in: pin it in the repo's `mise.toml` (`python = "3.13"`), otherwise the shim reports "command not found".
