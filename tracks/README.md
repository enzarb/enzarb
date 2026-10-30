# enzarb-tracks

Runs ez workflows (`.ez/workflows.toml`) for enzarb projects ([ADR-0029](../docs/adr/0029-ez-workflows.md)). It serves the API behind the UI's modes and workflow designers and, with routing on, routes every connector event to the owning Project's workflows and records a decision (rollout step 3). Runs are created `pending`; executing them is a later step.

All workflow semantics come from [`ez-workflow`](../ez/crates/ez-workflow) and all mode editing from ez-core (`modes_api`, `modes_edit`), so the UI never re-implements a rule.

## API

Cluster-internal. The UI calls it after its own access checks (write permission on the repository). Every `/v1` route needs `Authorization: Bearer $ENZARB_TRACKS_TOKEN`. Errors are `{"error": {"code", "message"}}`. Envelopes and workflow objects are snake_case; mode documents keep ez-core's camelCase (`ez modes --json`).

| Route | Request | Response |
|---|---|---|
| `GET /healthz`, `GET /readyz` | | `ok` (no auth) |
| `GET /v1/modes/catalog` | | ez's built-in tools, hook events, permission and `on_failure` values, and the `ez init` templates (`Catalog`; mise tasks, MCP servers and profiles are empty) |
| `POST /v1/modes/parse` | `{text}`: a `.ez/modes.toml` | `{doc: ModesDoc, issues}`, every mode with `originalId` = its id (keep it when renaming, so render rewrites the section in place); 422 `invalid_modes_toml` when it doesn't parse |
| `POST /v1/modes/validate` | `{doc}` | `{issues}` (`severity`: `error` blocks saving, `warning`, `info`) |
| `POST /v1/modes/render` | `{old?, doc}`: `old` is the file the editor loaded | `{text, issues, rewritten}`: the new file, changing only what was edited (comments kept); 422 `invalid_modes` (with `issues`) when the doc has errors, so an invalid file is never produced |
| `POST /v1/workflows/check` | `{workflows_toml, modes_toml?}` | `{report: {diagnostics: [{severity, path, message}]}, file?}`; `file` is the parsed document for the graph designer |
| `POST /v1/workflows/render` | `{file, modes_toml?}` | `{text, report}` (comments are not preserved) |
| `POST /v1/dry-run` | `{workflows_toml, modes_toml?, events?: [{id, event, action?, payload}], event_ids?: [id], options?: {workflow?, assume?, max_steps?}}` (1-50 events in total, rows of `connectors.event_v`; `event_ids` are read from the database, so they need routing on, and the caller must already have checked they belong to the Project) | `{report, results: [{event_id, triggers, traces}]}`; `results` is empty when the file has errors |
| `GET /v1/decisions?event_ids=1,2,3` | 1-200 ids | `{decisions: [{event_id, repo, project, sha, outcome, matched, reasons, errors, decided_at}]}`; `outcome` is `started`, `no_match`, `skipped` or `error`; `reasons` is `[{workflow, matched, on, reason}]`; events with no decision yet are absent. 503 `routing_disabled` without routing |

`modes_toml` is the `.ez/modes.toml` of the same commit; leave it out when the commit has none (agent nodes then fail validation, ADR-0028 "No defaults"). A dry run uses no sandbox, writes nothing to GitHub and calls no model: where it needs an outcome (an agent's result, a command's exit, a wait's event) it stops with `needs_outcome`, unless `options.assume` gives that node's outputs.

## Configuration

| Variable | Default | |
|---|---|---|
| `ENZARB_TRACKS_TOKEN` | (required) | Bearer token shared with the UI, at least 32 bytes |
| `ENZARB_TRACKS_LISTEN` | `0.0.0.0:8080` | |
| `ENZARB_TRACKS_MAX_BODY_BYTES` | 8 MiB | Request body limit (at least 64 KiB) |
| `ENZARB_TRACKS_ROUTING` | `false` (chart: `tracks.routing.enabled`, default on) | Route connector events; needs Postgres |
| `DATABASE_URL`, `PG*` | | Role `enzarb_tracks` (routing) / `enzarb_tracks_owner` (`enzarb-tracks migrate`) |
| `ENZARB_GITHUB_APP_ID`, `ENZARB_GITHUB_APP_PRIVATE_KEY` | | GitHub App used to read `.ez/*.toml` at the default branch head; without it every event gets an `error` decision |
| `ENZARB_TRACKS_CONCURRENCY` | 4 | Events routed at once (1-64) |
| `ENZARB_TRACKS_SWEEP_SECS` | 30 | Catch-up sweep interval |
| `RUST_LOG` | `info` | JSON logs |

## Routing

`LISTEN connector_event` -> read `connectors.event_v` -> find the Project listing the repo (`get`/`list` on Project CRs) -> read `.ez/workflows.toml` and `.ez/modes.toml` at the default branch head (sha pinned) through the GitHub App -> evaluate every workflow's `on` / `if` with `ez-workflow` -> one transaction writes `tracks.event_decisions` and a `pending` `tracks.runs` row per match. `event_decisions.event_id` is the primary key, so an event is decided at most once; a sweep after every (re)connect and periodically catches up events missed while down (everything past `tracks.router_state.floor`, set to the newest event id the first time tracks runs). Schema: [contracts.md](../docs/contracts.md) section 6a. Migrations: `enzarb-tracks migrate` (the chart's `enzarb-migrate-tracks` Job).

## Checks

```sh
cargo clippy -p enzarb-tracks --all-targets -- -D warnings && cargo test -p enzarb-tracks   # tests/routing.rs needs Docker (Postgres 18)
docker build -f tracks/Dockerfile .   # from the repo root
```
