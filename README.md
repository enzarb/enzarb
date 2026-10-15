# enzarb

The `enzarb` monorepo: AI coding agents running in Kubernetes sandboxes against code repositories, triggered from the UI or from events such as GitHub issues and PRs.

- [docs/architecture.md](docs/architecture.md): components, diagrams, key flows
- [docs/workflows.md](docs/workflows.md): the in-house Postgres workflow engine
- [docs/connectors.md](docs/connectors.md): connector contract (GitHub first)
- [docs/database.md](docs/database.md): shared Postgres via pgop
- [docs/mvp.md](docs/mvp.md): MVP scope and execution waves
- [docs/brand.md](docs/brand.md): brand identity, palette, typography and logo usage
- [docs/contracts.md](docs/contracts.md): cross-component interfaces (CRD, protocol messages, events, DB API, config)
- [docs/dev.md](docs/dev.md): local development on kind (`mise run dev:helm-deploy`), GitHub App setup, first dogfood, opt-in git hooks (`mise run hooks:install` once per clone)
- [docs/github-app.yml](docs/github-app.yml): GitHub App manifest
- [docs/ci.md](docs/ci.md): CI conventions (caching, parallel jobs, build-once images)
- [docs/gaps.md](docs/gaps.md): open questions
- [docs/adr/](docs/adr/): decision records

Each component lives in its own top-level directory (`proto/`, `operator/`, `connectors/`, `workflows/`, `realtime/`, `sandbox-proxy/`, `sandbox-container/`, `ui/`, `updater/`, `kubernetes/`, `helm/`) with its own checks and release tags `<dir>/vX.Y.Z`; see [CLAUDE.md](CLAUDE.md) and [ADR-0015](docs/adr/0015-monorepo.md).
