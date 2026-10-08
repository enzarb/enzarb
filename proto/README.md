# enzarb-proto

Mux protocol for enzarb: frame codec (CTRL/DATA_BIN/DATA_JSON/WINDOW/CLOSE), credit-based flow control, Rust crate + TS package, shared test vectors.

See [docs/architecture.md](https://github.com/enzarb/enzarb/blob/main/docs/architecture.md) in the top-level `enzarb` repo.

## Layout

- `docs/protocol.md`: wire format spec.
- `crates/enzarb-proto`: Rust codec (`Frame`), typed `Ctrl` messages, `FlowControl` state machine.
- `vectors/`: shared test vectors (Rust now, TypeScript later).

`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
