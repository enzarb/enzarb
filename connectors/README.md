# enzarb-connectors

Source-agnostic connectors for enzarb: the `Connector` trait, the GitHub connector, and the webhook `receiver` binary.

See [docs/architecture.md](https://github.com/enzarb/enzarb/blob/main/docs/architecture.md) in the top-level `enzarb` repo.

## Receiver

`receiver serve` (default) listens on `RECEIVER_LISTEN` (default `0.0.0.0:8080`); `receiver migrate` applies the `connectors` schema migrations (history in `connectors._sqlx_migrations`) and exits. Env: `DATABASE_URL`, `GITHUB_WEBHOOK_SECRET` (serve only), `ENZARB_BOT_HANDLE`, `ENZARB_BOT_LOGIN`, `RUST_LOG`.
