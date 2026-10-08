# enzarb

The top-level `enzarb` repository: AI coding agents running in Kubernetes sandboxes against code repositories, triggered from the UI or from events such as GitHub issues and PRs.

- [docs/architecture.md](docs/architecture.md): components, diagrams, key flows
- [docs/workflows.md](docs/workflows.md): the in-house Postgres workflow engine
- [docs/connectors.md](docs/connectors.md): connector contract (GitHub first)
- [docs/database.md](docs/database.md): shared Postgres via pgop
- [docs/mvp.md](docs/mvp.md): MVP scope and execution waves
- [docs/contracts.md](docs/contracts.md): cross-repo interfaces (CRD, protocol messages, events, DB API, config)
- [docs/gaps.md](docs/gaps.md): open questions
- [docs/adr/](docs/adr/): decision records

Component repos are listed in `repos.toml`. `mise run repos:list` shows their local state; `mise run repos:init` clones or initializes them.
