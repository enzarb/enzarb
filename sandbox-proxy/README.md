# enzarb-sandbox-proxy

Routes a browser websocket to its Project's sandbox-init (contracts §7).

`GET /sandbox/v1/projects/{project}/mux?token=<jwt>` and `GET /healthz` on `:8080`.

## Request flow

1. Validate the JWT (HS256, `ENZARB_SESSION_JWT_SECRET`, `aud: enzarb-sandbox-proxy`, `exp`, `project` claim equals the path; `exp` more than 5 minutes ahead is refused). 401 invalid, 403 project mismatch.
2. Look up the `Project` (reflector cache): 404 unknown; `sub` must equal `spec.owner` (403); 503 if `status.sandbox` is not published yet.
3. Find the sandbox only through `status.namespace` and `status.sandbox` (ADR-0013; no derived names: a Project missing either is not ready, 503). Read the bearer token from `status.sandbox.authSecretRef` in `status.namespace` (`get`, cached 30 s) and dial `status.sandbox.endpoint` (`ws://` only) with `Authorization: Bearer`, handshaking as `client: "proxy"`. 502 on failure.
4. Upgrade the browser socket, answer its `hello` locally (must be `client: "ui"` offering the version the sandbox negotiated), then relay binary messages unchanged in both directions.
5. Access is re-validated live: if the Project's `spec.owner` changes, its `status.sandbox` is cleared or the Project is deleted, the open relay is closed with code 1008 "project access changed".

**Handshake choice: terminate, not relay.** The sandbox must see `client: "proxy"` and failures should be HTTP errors, so the proxy handshakes upstream first and answers the browser's hello itself. Frames after the handshake are untouched, so flow control stays end to end.

**Security boundary.** Every browser->sandbox message is fully decoded (`Frame::from_bytes`); CTRL is allowlisted to `open`, `resize` and `error`. `run_start`, `run_cancel`, `run_status`, `run_state`, a second `hello`, unknown or malformed CTRL, text messages and trailing/short frames all close the connection (CTRL `error` + close 1008/1002). The sandbox never sees them.

## Configuration

| Var | Default | |
|---|---|---|
| `ENZARB_SESSION_JWT_SECRET` | required | shared with the UI |
| `PROXY_LISTEN` | `0.0.0.0:8080` | |
| `PROXY_IDLE_TIMEOUT_SECS` | `2100` | no data traffic either way closes the socket (websocket pings and pongs do not count). Raised to `ENZARB_CHAT_IDLE_SECS` (default `1800`, read only to validate) with a warning if lower |
| `PROXY_PING_INTERVAL_SECS` | `30` | websocket ping to the browser, keeps intermediaries open for an idle chat |
| `PROXY_MAX_MESSAGE_BYTES` | `1048586` | one max frame (1 MiB + header) |
| `PROXY_SECRET_TTL_SECS` | `30` | auth token cache |
| `RUST_LOG` | `info` | JSON logs; the URL (token) is never logged |

## RBAC

ClusterRole on the proxy's ServiceAccount (Projects are cluster-scoped):

- `enzarb.dev` `projects`: `get, list, watch`

Role plus RoleBinding **in the shared sandbox namespace** (`enzarb-sandboxes` by default, ADR-0013), bound to the proxy's ServiceAccount in the release namespace:

- core `secrets`: `get` (no list/watch). `resourceNames` cannot pin it because Secret names are per Project (`sb-<project>-auth`); the namespace holds no other Secrets. No cluster-wide Secret access.

## Build

`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`. `docker build --secret id=proto_key,src=<key> --secret id=crds_key,src=<key> .` (the enzarb git dependencies are private; pass deploy keys as build secrets). Tools via `mise install`.

## Chat (`acp`) channels

The browser handshake is answered by the proxy: `welcome.features` is the sandbox's list intersected with `["acp"]`, so an old sandbox (no features) shows the browser none. `open{kind:"acp"}` is forwarded (original bytes) only if `acp` was advertised, `repo` <= 2048 and `resume` <= 256 bytes, and the socket has no other live `acp` channel. Otherwise the proxy answers `open_error` itself (`unsupported_kind`, `invalid_request`, `chat_limit`) and the socket stays up. Unknown kinds are never forwarded. `run_*` and other forbidden CTRL still close the socket with 1008.
