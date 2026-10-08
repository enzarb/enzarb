# 0003. Single websocket with a custom mux protocol (enzarb-proto)

## Context
A browser session needs several concurrent streams to one sandbox: terminals, the claude session, status events, file transfer. Opening one connection per stream complicates auth and routing.

## Decision
- **One browser websocket** → `enzarb-sandbox-proxy` → the sandbox's pid-1 (`sandbox-init`).
- **Custom binary mux protocol:** frame types `CTRL`, `DATA_BIN`, `DATA_JSON`, `WINDOW`, `CLOSE`, with **credit-based flow control** for each channel.
- **Its own repo, `enzarb-proto`:** the spec (`docs/protocol.md`), a Rust crate, a TS package, and shared test vectors. Four programs speak it: the browser, the proxy, sandbox-init, and the workflow executor, which uses it to start runs (`StartRun`/`RunFinished`).

## Consequences
- The proxy can parse frames, which allows per-channel authorization and metrics.
- Protocol changes are versioned and negotiated in the opening `CTRL` handshake.
- Reconnect and resume semantics are still open ([gaps.md](../gaps.md)).
