# enzarb `helm/`

The `helm/` component of the enzarb monorepo (github.com/enzarb/enzarb). The repo root holds the architecture docs and shared conventions: see `../CLAUDE.md` and `../docs/architecture.md`. Releases are tagged `helm/vX.Y.Z`.

Run this component's own build/test commands from this directory. Changes to shared contracts (`proto/` frames, `connectors.event_v` view, `app_api` schema) must stay backward compatible.

Never remove a values key in one release: deprecate it in `values.schema.json` (`DEPRECATED:` description, accepted and ignored). `mise run compat` checks the chart against the previous release's values; see `../docs/ci.md` ("Values compatibility").
