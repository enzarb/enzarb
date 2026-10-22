# enzarb `ez/`

The `ez/` component of the enzarb monorepo (github.com/enzarb/enzarb): enzarb's coding agent (ADR-0028). The repo root holds the architecture docs and shared conventions: see `../CLAUDE.md` and `../docs/architecture.md`. Releases are tagged `ez/vX.Y.Z`.

Run this component's own build/test commands from this directory: `cargo clippy --all-targets && cargo test`.

- `ez` speaks only ACP. The TUI, `ez -p` and `ez acp` are ACP clients/servers around the in-process agent (`crates/ez-acp`).
- Modes come from `.ez/modes.toml`; unattended enzarb runs will use the default-branch copy, pinned per run (ADR-0028). `docs/modes.md` is the user-facing reference for modes, tools, MCP servers, permissions and hooks: update it when you change any of them (tool schemas, limits, defaults, hook semantics).
- Keep the ACP wire shapes stable: sandbox-init and the UI consume them (`crates/ez-acp/tests/golden.rs`).
