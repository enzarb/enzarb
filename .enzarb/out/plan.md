## Summary
Make Rust the single source of truth for the `Ctrl` frame set. (1) Generate `proto/ts/src/ctrl.gen.ts` (types, the list of `type` tags, and a table of secret field paths) from `enzarb-proto`, with a CI staleness check. (2) Give `Ctrl` `tag()` and `allowed_from(ClientKind)` from one exhaustive table in `enzarb-proto`, and use them in `sandbox-proxy/src/guard.rs` and the session core. (3) Replace the `credential_captured` byte scan with a minimal `{type}` serde decode. Wire format does not change; the JSON vectors are the guard. One PR: proto → sandbox-proxy → ui (per CLAUDE.md order).

## Approach
**Direction table (proto).**
- Add `strum` (`IntoStaticStr`, or `AsRefStr`) on `Ctrl` for `tag() -> &'static str`. It must follow `rename_all = "snake_case"`: `#[strum(serialize_all = "snake_case")]`. A unit test checks `tag()` equals the serialized `type` for every vector, so strum and serde cannot drift.
- Add `Ctrl::allowed_from(&self, ClientKind) -> bool`. It is a single exhaustive `match` with no `_` arm, so a new variant does not compile until it is classified. Each arm names the allowed senders. The set of senders for a variant is: UI, Executor, Sandbox (server). I'll first check what `ClientKind` has (`ctrl.rs:12`) and whether "server → client" frames need a separate `Direction` enum. If so, the table is `fn direction(&self) -> Direction { ClientToServer(&'static [ClientKind]) | ServerToClient }`, and `allowed_from(kind)` is derived from it.
- Today's rules: UI may send `open`, `resize`, `error`, `credential_status`, `credential_clear`. `step_*` requests are executor-only (ADR-0022). Replies and sandbox-originated frames are server → client only. `hello` is the handshake only, so the guard denies it after the handshake.
- Replace `ctrl_name` in `session/core.rs:~700` with `tag()`. Fix the direction comment on `Event::Ctrl` (`core.rs:~84`) to point at the table. Add a test that the documented server-side and client-side event sets match the table.

**Guard (sandbox-proxy).**
- `ctrl_allowed` becomes `if !ctrl.allowed_from(ClientKind::Ui) { Err(ctrl.tag()) }`. Default-deny comes from the table.
- Delete `is_captured_type`. The fallback at `guard.rs:~247` decodes `#[derive(Deserialize)] struct TypeOnly<'a>{ #[serde(rename="type")] ty: &'a str }` from the payload and compares it with `"credential_captured"`.
- Behaviour change to flag: the byte scan also matched a captured frame that is not valid JSON or has a mangled `type`. The serde decode drops only payloads that parse as an object with that `type`. A payload that fails full `Ctrl` decode but has a valid `type` string is still dropped, which is the case the comment cares about (a malformed `token` field). Garbage that is not JSON is relayed as it is today for non-captured garbage. I'll check this against the existing guard tests and add cases for malformed token and non-JSON. If the maintainers prefer fail-closed, drop any CTRL frame from the sandbox that fails to decode. That is stricter, and I'd keep it only if no vector or test needs a pass-through.

**TS generation (tool choice).**
- ts-rs derives TS types but has no notion of "secret fields". schemars can carry a marker: `Secret` implements `JsonSchema` with a custom keyword (`"x-secret": true`), and the paths can be found by walking the schema. Neither tool alone does both well, and a full JSON-Schema→TS emitter is more code to own than a small secret walker.
- Decision: **ts-rs for types, schemars for the secret table.** Both are derived behind a new optional `codegen` feature of `enzarb-proto`, so runtime consumers (`sandbox-proxy`, `workflows`, `sandbox-container`) never compile them. `schemars` 1 is already in the workspace.
- `Secret` has a manual `TS` impl that renders as `SecretLike`, the existing type in `secret.ts`.
- The generator is an example, `crates/enzarb-proto/examples/gen_ts.rs`, run as `cargo run -p enzarb-proto --features codegen --example gen_ts`. It writes `ts/src/ctrl.gen.ts` containing:
  - the `Ctrl` union and the payload types it uses (`StepStart`, `StepFinished`, `SessionEvent`, `SandboxStats`, `StepPhase`, `CredentialKind`, …);
  - `CTRL_TYPES` (from the schema's `oneOf` tags), replacing `TYPES`;
  - `SECRET_PATHS: Record<string, string[][]>`, with `*` for map values and array items, e.g. `step_start: [["workspaces","*","token"],["env","*"],["files","*"],["github_token"]]`, `open: [["claude_token"]]`, `credential_captured: [["token"]]`.
- The file header says "generated, do not edit".
- Alternatives rejected: schemars-only (needs our own TS emitter); ts-rs-only with a hand secret list (that is the duplication we are removing); a bare regex over `Secret` fields (brittle).

**Hand-written TS.**
- `ctrl.ts` shrinks to what is not generated: `UnknownCtrlType`, `asCtrl`, `parseCtrl`, and the `Secret` wrapping. `asCtrl` walks `SECRET_PATHS[t]` generically, replacing the special-cased `wrapStepStart` and the `credential_captured`/`open` branch. It re-exports the generated types, so the `index.ts` exports and ui imports keep their names.
- `session.ts` stays hand-written and is covered by the vectors and `session.test.ts`.
- Any shape difference between the generated types and today's hand types (optional `?` vs `| null`, intersection `& StepStart` vs inlined fields, `bigint` for `u64`) is handled with ts-rs attributes (`#[ts(optional)]`, `#[ts(type = "number")]`) so the public type stays compatible. If something cannot be matched exactly, I'll list it in the PR and let the `ui` `npm run check` decide.

**CI staleness.**
- Add `mise run proto:gen` (`.mise/tasks/proto/gen`), which runs the generator, with a `--check` mode that regenerates and `git diff --exit-code`s `ctrl.gen.ts`.
- Add a step to the proto job in `.github/workflows/ci-proto.yml`. Update the lefthook pre-push entry and `docs/ci.md`, and `proto/README.md` / `proto/CLAUDE.md` ("adding a Ctrl variant").
- Check whether `scripts/monorepo/affected.sh` needs a change. It should not, since no `paths:` filter changes.

## Changes
- `proto/Cargo.toml`, `proto/crates/enzarb-proto/Cargo.toml`: add `strum` (and `strum_macros`, if needed); optional `ts-rs`/`schemars` under feature `codegen`.
- `proto/crates/enzarb-proto/src/ctrl.rs`: derive `strum`; `tag()`, `allowed_from()`, the direction table; `Secret` `TS` and `JsonSchema` impls and derives behind `codegen`.
- `proto/crates/enzarb-proto/src/step.rs` (and `ctrl.rs` types it uses): `cfg_attr(feature = "codegen", derive(TS, JsonSchema))`.
- `proto/crates/enzarb-proto/examples/gen_ts.rs`: the generator.
- `proto/crates/enzarb-proto/src/session/core.rs`: use `tag()`; fix the direction comment.
- `proto/ts/src/ctrl.gen.ts` (new, committed), `proto/ts/src/ctrl.ts`, `proto/ts/src/index.ts`.
- `sandbox-proxy/src/guard.rs` (+ its tests).
- `.mise/tasks/proto/gen`, `.github/workflows/ci-proto.yml`, `lefthook.yml` / `scripts/monorepo/pre-push.sh` if needed, `docs/ci.md`, `proto/README.md`, `proto/CLAUDE.md`.
- `ui/`: only fixes needed if type-checking against the regenerated package finds a difference. The `ui` lockfile should not change.

## Tests
- proto (Rust): `tag()` matches the serde tag for all vectors; the direction table is checked against the vectors (e.g. each variant's `allowed_from` for each `ClientKind`, with the UI set pinned to the 5 current variants); existing `vectors.rs` and session tests unchanged.
- proto (TS): `vectors.test.ts` keeps passing. Add a test that `CTRL_TYPES` equals the vector tag set and that `SECRET_PATHS`-driven wrapping yields `Secret` for every secret in the vectors (`open_claude_token`, `credential_captured_token`, and the `step_start` fields).
- sandbox-proxy: existing guard tests; add malformed/non-JSON `credential_captured` cases and a UI-allowed table test.
- Staleness: `mise run proto:gen -- --check` on a clean tree passes and fails after a deliberate Rust edit.
- Checks to run: `proto/`: `cargo test`, `cargo test --features codegen`, `npm test`, `npm run check`; `sandbox-proxy/`: `cargo clippy --all-targets && cargo test`; `ui/`: `npm run check && npm run lint && npm test`. Also `workflows/` and `sandbox-container/`, because they depend on proto: `cargo clippy --all-targets && cargo test`.

## Risks and open questions
- Generated TS may differ in shape from the hand-written types; ui type-check is the test. Wire stays unchanged.
- The `{type}` decode narrows the byte-scan's behaviour on non-JSON input (see above). Confirm fail-open vs fail-closed for undecodable sandbox CTRL frames.
- Two derive crates (ts-rs and schemars) is a deliberate trade-off. Both are behind `codegen`, but the generated output depends on their versions, so the lockfile is pinned and a version bump may regenerate the file.
- Placement of the direction table: it is protocol knowledge, so it lives in proto, not in sandbox-proxy.
- If the generator proves much larger than expected, split into (a) tag/`allowed_from` + guard refactor and (b) TS generation + CI check.

## Out of scope
- Generating `session.ts`, `frame.ts`, `flow.ts` or the other hand-written TS.
- Any wire or schema change, new frames, or changes to the `workflows_api`/`app_api` schemas.
- Generating the other direction of the docs (`docs/protocol.md`).
- Other `Ctrl` consumers in `workflows` / `sandbox-container` beyond what compilation requires.

Reply with feedback to revise this plan, or comment /approve to start the implementation.
