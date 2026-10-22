# enzarb `connectors/`

The `connectors/` component of the enzarb monorepo (github.com/enzarb/enzarb). The repo root holds the architecture docs and shared conventions: see `../CLAUDE.md` and `../docs/architecture.md`. Releases are tagged `connectors/vX.Y.Z`.

It is only the `receiver` binary (`bin/receiver`): it verifies GitHub webhooks and logs each delivery in `connectors.events`. It does no routing and makes no outbound calls. Check: `cargo clippy --all-targets && cargo test`.

Run this component's own build/test commands from this directory. The cross-component read surface is the view `connectors.event_v`; changes to it and to other shared contracts (`proto/` frames, `app_api`) must stay backward compatible.
