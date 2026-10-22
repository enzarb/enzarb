# enzarb-tracks

Runs ez workflows (`.ez/workflows.toml`) for enzarb projects ([ADR-0029](../docs/adr/0029-ez-workflows.md)). Today it is the stateless API behind the UI's modes and workflow designers; event routing, run state (Postgres) and real runs come in later rollout steps.

All workflow semantics come from [`ez-workflow`](../ez/crates/ez-workflow) and all mode editing from ez-core (`modes_api`, `modes_edit`), so the UI never re-implements a rule.

## API

Cluster-internal. The UI calls it after its own access checks (write permission on the repository). Every `/v1` route needs `Authorization: Bearer $ENZARB_TRACKS_TOKEN`. Errors are `{"error": {"code", "message"}}`. Envelopes and workflow objects are snake_case; mode documents keep ez-core's camelCase (`ez modes --json`).

| Route | Request | Response |
|---|---|---|
| `GET /healthz`, `GET /readyz` | | `ok` (no auth) |
| `GET /v1/modes/catalog` | | ez's built-in tools, hook events, permission and `on_failure` values, and the `ez init` templates (`Catalog`; mise tasks, MCP servers and profiles are empty) |
| `POST /v1/modes/parse` | `{text}`: a `.ez/modes.toml` | `{doc: ModesDoc, issues}`; 422 `invalid_modes_toml` when it doesn't parse |
| `POST /v1/modes/validate` | `{doc}` | `{issues}` (`severity`: `error` blocks saving, `warning`, `info`) |
| `POST /v1/modes/render` | `{old?, doc}`: `old` is the file the editor loaded | `{text, issues, rewritten}`: the new file, changing only what was edited (comments kept); 422 `invalid_modes` (with `issues`) when the doc has errors, so an invalid file is never produced |
| `POST /v1/workflows/check` | `{workflows_toml, modes_toml?}` | `{report: {diagnostics: [{severity, path, message}]}, file?}`; `file` is the parsed document for the graph designer |
| `POST /v1/workflows/render` | `{file, modes_toml?}` | `{text, report}` (comments are not preserved) |
| `POST /v1/dry-run` | `{workflows_toml, modes_toml?, events: [{id, event, action?, payload}], options?: {workflow?, assume?, max_steps?}}` (1-50 events, rows of `connectors.event_v`) | `{report, results: [{event_id, triggers, traces}]}`; `results` is empty when the file has errors |

`modes_toml` is the `.ez/modes.toml` of the same commit; leave it out when the commit has none (agent nodes then fail validation, ADR-0028 "No defaults"). A dry run uses no sandbox, writes nothing to GitHub and calls no model: where it needs an outcome (an agent's result, a command's exit, a wait's event) it stops with `needs_outcome`, unless `options.assume` gives that node's outputs.

## Configuration

| Variable | Default | |
|---|---|---|
| `ENZARB_TRACKS_TOKEN` | (required) | Bearer token shared with the UI, at least 32 bytes |
| `ENZARB_TRACKS_LISTEN` | `0.0.0.0:8080` | |
| `ENZARB_TRACKS_MAX_BODY_BYTES` | 8 MiB | Request body limit (at least 64 KiB) |
| `RUST_LOG` | `info` | JSON logs |

## Checks

```sh
cargo clippy -p enzarb-tracks --all-targets -- -D warnings && cargo test -p enzarb-tracks
docker build -f tracks/Dockerfile .   # from the repo root
```
