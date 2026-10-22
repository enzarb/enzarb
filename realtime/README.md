# enzarb-realtime

Realtime change stream for the enzarb UI. One Postgres connection `LISTEN`s on `connector_event` (sent by the connectors receiver for each logged webhook delivery); each notification is mapped to the event's repository and pushed to the browsers that may see that repository over a websocket. The UI then refetches via its remote functions. No state, one binary, one replica is enough (each replica holds its own LISTEN connection, so more replicas also work).

`GET /live/v1/stream` (websocket), `GET /healthz`, `GET /readyz` on `:8080`.

## Stream

- **Token sources:** the JWT is read from the first source that is *present*, in this order, with no fall-through (a present but invalid or malformed source is a 401, even if a later source would be valid):
  1. `Authorization: Bearer <jwt>` (non-browser clients; scheme case-insensitive). Any other `Authorization` value, or an empty token, is a 401. No `Origin` needed.
  2. the `enzarb_live` cookie (browsers: the page's origin sends it on the upgrade automatically). The first `enzarb_live` pair is used, verbatim. Needs an allowed `Origin` (below), checked before the token, else **403**. The cookie's `HttpOnly` / `Secure` / `SameSite` / `Path=/live/` attributes are set by whatever issues it, not by this service.
  3. `?token=<jwt>` in the query string: **deprecated**, it ends up in logs and history. Each successful use logs a warning with `deprecated_total` (a counter, no token) and the `sub`. Remove it once enzarb-ui#71 is released and the counter has stayed at 0 for 7 days in the oldie logs; only then open the removal issue.
- **Origin check (cookie auth only, CSWSH protection):** a missing or disallowed `Origin` gets 403 before the upgrade. The `Origin` must be exactly `scheme://host[:port]` (`http`/`https`); `null`, a path (even `/`), query, fragment or userinfo is rejected. Scheme and host compare case-insensitively, the port exactly (`https://h` and `https://h:443` differ; browsers omit default ports, so list `https://host`). With `ENZARB_REALTIME_ALLOWED_ORIGINS` set, the `Origin` must equal an entry. When unset it must match the request `Host` header (only that header; `X-Forwarded-Host` is ignored). The Host default is for dev: behind Cloudflare Tunnel / Envoy `Host` may be rewritten, so **production must set the allowlist**.
- **Auth:** HS256, secret `ENZARB_REALTIME_JWT_SECRET`, claims `sub`, `repos` (array of lowercase `owner/repo` keys of the repositories of the projects the user can see), `projects` (array of project names; accepted, not used for filtering), `aud: "enzarb-realtime"`, `exp`. Signature, audience and `exp` are checked at connect (no leeway) -> 401 otherwise. If `iat` is present, `exp - iat` must be at most `ENZARB_REALTIME_MAX_TOKEN_TTL_SECS` (300). The socket never outlives `min(exp, now + max ttl)`.
- **Expiry:** at that point the server closes with code **4001** (`token expired; reconnect with a fresh token`). The client fetches a new token and reconnects, then refetches (treat a reconnect like `resync`).
- **Frames** (server -> client, JSON text):
  - `{"topic":"event","id":123,"repo":"enzarb/enzarb"}`, only for repositories in the token's `repos`;
  - `{"topic":"resync"}`, sent to everyone: refetch everything.
- **Resync is sent when:** the LISTEN connection was lost and re-established (changes may have been missed), an event's repository cannot be resolved (lookup failed, no row, or the delivery names no repository), or a client's buffer overflowed (`ENZARB_REALTIME_BUFFER` frames; the client lagged).
- **Keepalive:** a ping every 15 s; a client that has not ponged for 1.5 intervals is dropped (close **1001** `ping timeout`). Client messages are ignored.
- **Close codes:** **1001** on graceful shutdown (SIGTERM) and ping timeout, **1011** on an internal error, **4001** on token expiry. Reconnect on any of them.
- **Connection cap:** at most `ENZARB_REALTIME_MAX_CONNECTIONS` concurrent websockets; beyond that (after auth) the upgrade is refused with **503** and `Retry-After: 1`, logged and counted.
- `/readyz` is 200 only while the LISTEN connection is up. `/healthz` is always 200.

## Database

`DATABASE_URL` is the same pattern as the other services (`postgres:///enzarb` plus `PG*` env vars; see `docs/database.md` in the workspace). The runtime role needs:

- `CONNECT` on the database, `USAGE` on schema `connectors`, and `SELECT` on **`connectors.event_v`** (the service reads only `id` and `repo`). Nothing else. The connectors migration grants it when the role exists.
- `LISTEN` needs no grant.

TLS (rustls, ring backend, bundled webpki roots) is controlled by `sslmode` / `PGSSLMODE` and `sslrootcert` / `PGSSLROOTCERT`:

- the default `sslmode=prefer` uses TLS when the server offers it, without verifying the certificate (previously it fell back to plaintext);
- `verify-ca` / `verify-full` validate against the bundled public roots, so certificates from a public CA need no `sslrootcert`;
- a private CA (e.g. cert-manager) needs `sslrootcert` (or `PGSSLROOTCERT`) pointing at a CA file mounted into the container.

The NOTIFY payload is only the event id, so each notification costs one `SELECT repo FROM connectors.event_v WHERE id = $1` on a 2-connection pool.

## Configuration

| Var | Default | |
|---|---|---|
| `DATABASE_URL` | required | Postgres connection (`PG*` env fills the rest); TLS via `sslmode` / `sslrootcert`, see Database |
| `ENZARB_REALTIME_JWT_SECRET` | required | HS256 secret shared with the UI; at least 32 bytes or startup fails |
| `ENZARB_REALTIME_LISTEN` | `0.0.0.0:8080` | |
| `ENZARB_REALTIME_PING_SECS` | `15` | ping interval |
| `ENZARB_REALTIME_BUFFER` | `256` | per-client buffered frames before a `resync` replaces them |
| `ENZARB_REALTIME_MAX_TOKEN_TTL_SECS` | `300` | max token lifetime / socket lifetime |
| `ENZARB_REALTIME_MAX_CONNECTIONS` | `1000` | max concurrent websockets, must be > 0; over the cap gets 503 |
| `ENZARB_REALTIME_ALLOWED_ORIGINS` | unset | comma-separated `scheme://host[:port]` allowed to use cookie auth, e.g. `https://enzarb.example`; unset means the `Origin` must match `Host` (dev only) |
| `RUST_LOG` | `info` | JSON logs |

The listener reconnects with exponential backoff (500 ms to 30 s).

## Build

`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`, `docker build .`. Tools via `mise install`. `tests/postgres.rs` uses testcontainers and skips itself without Docker.
