# enzarb-realtime

Realtime change stream for the enzarb UI. One Postgres connection `LISTEN`s on `work_item_changed`; each notification is mapped to its project and pushed to the browsers that may see that project over a websocket. The UI then refetches via its remote functions. No state, one binary, one replica is enough (each replica holds its own LISTEN connection, so more replicas also work).

`GET /live/v1/stream?token=<jwt>` (websocket), `GET /healthz`, `GET /readyz` on `:8080`.

## Stream

- **Auth:** browsers cannot set websocket headers, so the JWT goes in the query string. HS256, secret `ENZARB_REALTIME_JWT_SECRET`, claims `sub`, `projects` (array of project names the user can see), `aud: "enzarb-realtime"`, `exp`. Signature, audience and `exp` are checked at connect (no leeway) -> 401 otherwise. If `iat` is present, `exp - iat` must be at most `ENZARB_REALTIME_MAX_TOKEN_TTL_SECS` (300). The socket never outlives `min(exp, now + max ttl)`.
- **Expiry:** at that point the server closes with code **4001** (`token expired; reconnect with a fresh token`). The client fetches a new token and reconnects, then refetches (treat a reconnect like `resync`).
- **Frames** (server -> client, JSON text):
  - `{"topic":"work_item","id":123,"project":"enzarb"}`, only for projects in the token's `projects`;
  - `{"topic":"resync"}`, sent to everyone: refetch everything.
- **Resync is sent when:** the LISTEN connection was lost and re-established (changes may have been missed), a work item's project cannot be resolved (lookup failed or no row), or a client's buffer overflowed (`ENZARB_REALTIME_BUFFER` frames; the client lagged).
- **Keepalive:** a ping every 15 s; a client that has not ponged for 1.5 intervals is dropped (close **1001** `ping timeout`). Client messages are ignored.
- **Close codes:** **1001** on graceful shutdown (SIGTERM) and ping timeout, **1011** on an internal error, **4001** on token expiry. Reconnect on any of them.
- **Connection cap:** at most `ENZARB_REALTIME_MAX_CONNECTIONS` concurrent websockets; beyond that (after auth) the upgrade is refused with **503** and `Retry-After: 1`, logged and counted.
- `/readyz` is 200 only while the LISTEN connection is up. `/healthz` is always 200.

## Database

`DATABASE_URL` is the same pattern as the other services (`postgres:///enzarb` plus `PG*` env vars; see `docs/database.md` in the workspace). The runtime role needs:

- `CONNECT` on the database, `USAGE` on schema `workflows_api`, and `SELECT` on **`workflows_api.work_item_project_v (id, project)`** (a view exposing only those two columns; the service reads only `project`). Nothing else. The view ships with enzarb-workflows#40; deploy that migration before this service.
- `LISTEN` needs no grant.

The NOTIFY payload is only the work item id, so each notification costs one `SELECT project FROM workflows_api.work_item_project_v WHERE id = $1` on a 2-connection pool.

## Configuration

| Var | Default | |
|---|---|---|
| `DATABASE_URL` | required | Postgres connection (`PG*` env fills the rest) |
| `ENZARB_REALTIME_JWT_SECRET` | required | HS256 secret shared with the UI; at least 32 bytes or startup fails |
| `ENZARB_REALTIME_LISTEN` | `0.0.0.0:8080` | |
| `ENZARB_REALTIME_PING_SECS` | `15` | ping interval |
| `ENZARB_REALTIME_BUFFER` | `256` | per-client buffered frames before a `resync` replaces them |
| `ENZARB_REALTIME_MAX_TOKEN_TTL_SECS` | `300` | max token lifetime / socket lifetime |
| `ENZARB_REALTIME_MAX_CONNECTIONS` | `1000` | max concurrent websockets, must be > 0; over the cap gets 503 |
| `RUST_LOG` | `info` | JSON logs |

The listener reconnects with exponential backoff (500 ms to 30 s).

## Build

`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`, `docker build .`. Tools via `mise install`. `tests/postgres.rs` uses testcontainers and skips itself without Docker.
