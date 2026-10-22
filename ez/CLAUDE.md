# enzarb `ez/`

The `ez/` component of the enzarb monorepo (github.com/enzarb/enzarb): enzarb's coding agent (ADR-0028). The repo root holds the architecture docs and shared conventions: see `../CLAUDE.md` and `../docs/architecture.md`. Releases are tagged `ez/vX.Y.Z`.

ez's crates are members of the root Cargo workspace. Run its checks from the repo root: `cargo clippy -p ez-acp -p ez-claude -p ez-cli -p ez-core -p ez-hooks -p ez-llm -p ez-mcp -p ez-mise -p ez-tools -p ez-workflow --all-targets && cargo test -p ez-acp -p ez-claude -p ez-cli -p ez-core -p ez-hooks -p ez-llm -p ez-mcp -p ez-mise -p ez-tools -p ez-workflow`.

- `ez` speaks only ACP. The TUI, `ez -p` and `ez acp` are ACP clients/servers around the in-process agent (`crates/ez-acp`).
- Modes come from `.ez/modes.toml`; unattended enzarb runs will use the default-branch copy, pinned per run (ADR-0028). `docs/modes.md` is the user-facing reference for modes, tools, MCP servers, permissions and hooks: update it when you change any of them (tool schemas, limits, defaults, hook semantics).
- Keep the ACP wire shapes stable: sandbox-init and the UI consume them (`crates/ez-acp/tests/golden.rs`).
