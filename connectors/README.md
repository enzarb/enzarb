# enzarb-connectors

The webhook `receiver` binary for enzarb. It logs GitHub webhook deliveries and does nothing else with them ([docs/connectors.md](https://github.com/enzarb/enzarb/blob/main/docs/connectors.md), [ADR-0027](https://github.com/enzarb/enzarb/blob/main/docs/adr/0027-workflow-clean-slate.md)).

See [docs/architecture.md](https://github.com/enzarb/enzarb/blob/main/docs/architecture.md) in the top-level `enzarb` repo.

## Receiver

`POST /hooks/github` verifies `X-Hub-Signature-256` against `GITHUB_WEBHOOK_SECRET`, inserts one row into `connectors.events` (unique on `(source, delivery_id)`, so a redelivery is a no-op) and runs `pg_notify('connector_event', id)`. `GET /healthz` and `GET /readyz` are the probes. Other components read the view `connectors.event_v` only.

Subcommands:
- `receiver serve` (default): listens on `RECEIVER_LISTEN` (default `0.0.0.0:8080`).
- `receiver migrate`: applies the `connectors` schema migrations (history in `connectors._sqlx_migrations`) and exits.
- `receiver drop-legacy`: drops the schemas `workflows`, `workflows_api`, `engine` and `engine_api` of earlier releases (idempotent; the chart runs it first, as `enzarb_workflows_owner`).

Env: `DATABASE_URL`, `GITHUB_WEBHOOK_SECRET` (serve only), `RECEIVER_LISTEN`, `RUST_LOG`.
