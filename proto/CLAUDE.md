# enzarb `proto/`

The `proto/` component of the enzarb monorepo (github.com/enzarb/enzarb). The repo root holds the architecture docs and shared conventions: see `../CLAUDE.md` and `../docs/architecture.md`. Releases are tagged `proto/vX.Y.Z`.

Run this component's own build/test commands from this directory. Changes to shared contracts (`proto/` frames, `workflows_api`/`app_api` schemas) must stay backward compatible.

## Adding or changing a `Ctrl` variant

Rust (`crates/enzarb-proto/src/ctrl.rs`) is the single source of truth. Edit only the enum and its `Direction` table:

1. Add the variant to `Ctrl` (and its payload type, with `cfg_attr(feature = "codegen", derive(ts_rs::TS, schemars::JsonSchema))`). A field holding a token is a `Secret`.
2. Classify it in `Ctrl::direction()`: it is an exhaustive `match`, so it does not compile until you do. `tag()` and `allowed_from()` (used by sandbox-proxy's guard and the session core) derive from it.
3. Add a vector to `crates/enzarb-proto/tests/vectors.rs` and regenerate `vectors/`.
4. Run `mise run proto:gen` and commit `ts/src/ctrl.gen.ts` (types, `CTRL_TYPES`, `SECRET_PATHS`). CI (`mise run proto:gen -- --check`) fails on a stale file. No hand edit is needed in `ts/src/ctrl.ts` or in `sandbox-proxy`.

`ts/src/session.ts` stays hand-written; the vectors test it.
