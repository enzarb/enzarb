# Connectors (`connectors/`)

`connectors/` is the `receiver` binary: the one place GitHub webhooks arrive. It logs every delivery and does nothing else with it. The earlier `Connector` trait, normalized event catalog, outbound calls (comments, PRs, merges, reactions) and the engine that consumed them were removed ([ADR-0027](adr/0027-workflow-clean-slate.md)); how events should trigger work is to be redesigned.

The name `connectors` was chosen to avoid terms that already mean something in Kubernetes (ingress, gateway, router, proxy, controller, scheduler).

## Receiver

`POST /hooks/github`:
1. Verifies `X-Hub-Signature-256` (HMAC-SHA256 with `GITHUB_WEBHOOK_SECRET`); 401 otherwise. A missing `X-GitHub-Event` / `X-GitHub-Delivery` header or a non-JSON body is 400.
2. In one transaction, inserts a row into `connectors.events` and runs `pg_notify('connector_event', '<id>')`. A redelivery (same delivery id) is a no-op and sends no notification. Answers 202; 500 if the insert fails (GitHub can redeliver).

`GET /healthz` is liveness, `GET /readyz` checks the database.

## The event log

`connectors.events`:

| Column | Notes |
|---|---|
| `id` | bigserial |
| `source` | `github` |
| `delivery_id` | `X-GitHub-Delivery`; unique with `source` |
| `event` | `X-GitHub-Event` (`issues`, `pull_request`, `ping`, …) |
| `action` | payload `action`, may be NULL |
| `repo` | lowercase `owner/repo` from `repository.full_name`; NULL when the payload names no repository (ping, installation) |
| `sender` | `sender.login` |
| `payload` | the full JSON payload |
| `received_at` | |

Other components read only the view `connectors.event_v` (same columns): `enzarb_ui` and `enzarb_realtime` have `SELECT` on it. The UI lists a project's events by the repositories linked to the Project; realtime pushes `event` frames for them ([contracts.md](contracts.md) section 7a).

## Subcommands and configuration
- `receiver serve` (default): `RECEIVER_LISTEN` (default `0.0.0.0:8080`), `DATABASE_URL`, `GITHUB_WEBHOOK_SECRET`.
- `receiver migrate`: applies the `connectors` schema migrations and exits (chart migration Job).
- `receiver drop-legacy`: drops the schemas `workflows`, `workflows_api`, `engine`, `engine_api`; idempotent. It is the chart's first migration Job (`legacy`), run as the old owner role `enzarb_workflows_owner`, which stays the pgop Database owner. Migration order: legacy, connectors, ui.

## Testing
GitHub cannot reach a local kind cluster, so live testing goes through a smee-style relay (`mise run dev:webhook-relay`, [dev.md](dev.md)).
