# enzarb-proto

Mux protocol for enzarb: frame codec (CTRL/DATA_BIN/DATA_JSON/WINDOW/CLOSE), credit-based flow control, an async session runtime, a Rust crate and a TS package, and shared test vectors.

See [docs/architecture.md](https://github.com/enzarb/enzarb/blob/main/docs/architecture.md) in the top-level `enzarb` repo.

## Layout

- `docs/protocol.md`: wire format spec and session rules.
- `crates/enzarb-proto`: Rust codec (`Frame`, `FrameHeader`), typed `Ctrl` messages, `FlowControl`, and (feature `tokio`) the `session` runtime.
- `ts/`: the `@enzarb/proto` TypeScript package (codec, CTRL types, flow control, browser `Session`).
- `vectors/`: test vectors shared by the Rust and TS suites.

## Rust

```toml
enzarb-proto = { path = "../proto/crates/enzarb-proto", features = ["tokio"] }  # from a sibling component dir
```

Without features you get the pure codec, `Ctrl` and `FlowControl`. Feature `tokio` adds `enzarb_proto::session`; feature `tls` additionally lets `Client::connect` dial `wss://`.

| Consumer | Use |
|---|---|
| sandbox-init (server) | `Session::accept(ws, ServerOptions::default())`, then loop over `Events`: `Event::Open(req)` -> `req.accept()` gives a `ChannelHandle` (`AsyncRead + AsyncWrite`) for the pty; `Event::Ctrl(Ctrl::RunStart(..))` -> `session.accept_run(..)` (returns the transcript channel when `log_channel` is set), `session.send_run_state(..)` for `run_status` replies and pushes. |
| workflows executor (client) | `Client::connect(url, token, ClientKind::Executor)`, then `session.run_start_logged(run)` / `run_start(run)`, `session.run_status(id)`, `session.run_cancel(id)`; read the transcript with `log.recv.recv_json::<T>()`. |
| sandbox-proxy | Mostly raw forwarding: `session::handshake::server_handshake` on the browser socket, `client_handshake` on the sandbox socket, then relay binary messages, using `FrameHeader::parse` for routing. Flow control is end to end, so unmodified frames keep it intact. |

The runtime enforces the session rules for you: hello first, a channel table (ids are client chosen; reuse is refused), automatic `WINDOW` when the application reads, `error` replies for violations, and the `log_channel` lifecycle (opened by `run_accepted`). Writers wait for credit (backpressure, never drops). See the module docs of `enzarb_proto::session` for full server and client examples; `session::memory` provides an in-memory websocket for tests.

## TypeScript

Consumed inside the monorepo as a file dependency (ADR-0015); nothing is published to a registry. In `ui/package.json`:

```json
"@enzarb/proto": "file:../proto"
```

`ui`'s `proto:build` script (run by `prepare`) builds `ts/` into `dist/` before the UI build.

```ts
import { Session } from "@enzarb/proto";

const s = await Session.connect(`wss://host/sandbox/v1/projects/${project}/mux?token=${jwt}`, { client: "ui" });
const pty = await s.openPty({ cols: 80, rows: 24 });
await pty.write("ls\n");                       // waits for credit
for await (const d of pty) term.write(d.data); // credit is returned as you read
s.resize(pty, 120, 40);

const run = await s.runStart({ run_id: "job-1", /* ... */ log: true });
for await (const d of run.log!) console.log(JSON.parse(new TextDecoder().decode(d.data)));
s.credentialClear();                               // client -> server credential_clear
s.onCtrl((m) => { /* pushed run_state, credential_captured, unattributed errors */ });
```

ACP over a byte channel (only when `s.features.has("acp")`; an old proxy closes the socket on an unknown kind):

```ts
import { ndJsonStream } from "@agentclientprotocol/sdk";
const bc = await s.openAcp({ repo, resume });   // OpenError.openCode on refusal
const stream = ndJsonStream(bc.writable, bc.readable);
bc.closed.then(({ reason }) => { /* idle | superseded | shutdown | limit | exit:<n> | ... */ });
```

The TS `Session` is the client role only (ui, and Node clients). The codec (`decodeFrame`, `encodeFrame`, `parseCtrl`, `FrameDecoder`) and `FlowControl` are exported for other uses.

## Checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
npm ci && npm run check && npm test
```

Regenerate vectors with `ENZARB_UPDATE_VECTORS=1 cargo test -p enzarb-proto --test vectors`.
