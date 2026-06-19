# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Tooling

This repo uses [mise](https://mise.jdx.dev/) for tool version management and task running. All commands below assume `mise` is available.

```bash
mise run <task>      # run a defined task
mise run lint        # run all linters (no staged-file filter)
mise run install-hooks  # install git pre-commit hook
```

## Build

```bash
mise run build-operator   # go build ./cmd/operator/
mise run build-agent      # cargo build --release (in agent/)
mise run build-app        # npm run build (in app/)
mise run build-all        # all three
```

## Test

```bash
mise run test-operator    # go test ./...  (in operator/)
mise run test-agent       # cargo test (in agent/)
mise run test-all
```

## Dev

```bash
mise run dev-app          # SvelteKit vite dev server
```

## Lint (pre-commit checks)

The pre-commit hook (`scripts/pre-commit`) runs scoped checks based on staged files:

- **Go** (`operator/`, `metering/`, `billing/`): `gofmt`, `golangci-lint`, `govulncheck`
- **Rust** (`agent/`): `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo audit`
- **JS** (`app/`): `npm ci`, `svelte-check`
- **Containers** (`Dockerfile`, `.trivyignore`): `trivy config` (CRITICAL/HIGH)

`STAGED_OVERRIDE=all` bypasses the staged-file filter (used by `mise run lint`).

## Architecture

Enzarb is a Kubernetes-native AI agent development platform. It has five components:

### `operator/` (Go, controller-runtime)
A Kubernetes operator managing two CRDs (`enzarb.io/v1alpha1`):
- **Project** — represents a developer workspace; the controller provisions a namespace (`user-<orgId>`), ServiceAccount, PVC (`<slug>-home`), Deployment (runs the workspace image + project-agent sidecar), Service, and a Gitea repo. Phase/conditions tracked in `.status`.
- **Environment** — represents a deployment target with custom domains and cert-manager TLS integration.

Reconcilers live in `operator/internal/controller/`. The operator embeds an admin CLI (`operator/internal/admin/`). Config is passed via env vars; Gitea client lives in `operator/internal/gitea/`.

### `agent/` (Rust, tokio + axum)
The `project-agent` binary runs inside each workspace Pod. It exposes two HTTP servers:
- **:9090 internal** — health/status endpoint used by the operator/sidecar (`internal.rs`)
- **:8080 external** — JWT-authenticated API for the frontend (`external/`)

External API handles: terminal sessions (`terminal.rs`, `tmux.rs` using `portable-pty`), file access (`external/files.rs`), process management (`external/processes.rs`), and filesystem watching (`external/watch.rs`). JWT validation uses a JWKS cache fetched from `https://enzarb.dev/.well-known/jwks.json`.

`docker-credential-k8s-sa` is a second binary that implements a Docker credential helper using the pod's K8s ServiceAccount token (for pulling from the in-cluster Zot registry).

On first boot, `init.rs` writes `mise.toml` if absent and runs `mise install`.

### `app/` (SvelteKit + TypeScript)
SSR frontend using SvelteKit (adapter-node). Key libs:
- `src/lib/k8s.ts` — direct K8s API calls (operator CRDs, etc.)
- `src/lib/gitea.ts`, `src/lib/zot.ts` — Gitea and Zot OCI registry clients
- `src/lib/db.ts` — Postgres (via `postgres` package)
- `src/lib/jwt.ts`, `src/lib/session.ts` — JWT issuance and session management (OIDC via `openid-client`)
- `src/remote/` — thin typed wrappers the frontend uses to call the agent's external API
- Route layout: `(app)/orgs/[org]/projects/[project]/...` with sub-pages for terminal, files, registry, deployments, billing

The app issues its own JWTs (JWKS at `/.well-known/jwks.json`) which the agent validates, tying user identity to workspace access.

### `billing/` and `metering/` (Go)
Standalone services (`cmd/billing/`, `cmd/metering/`). Billing integrates with the River job queue (PostgreSQL-backed). Both communicate via Postgres.

### Infrastructure (`charts/`, `deploy/`)
Helm chart at `charts/enzarb/` deploys all components. System dependencies (Dex OIDC, Zot registry, Gitea) are deployed via `deploy/system/` as HelmChart CRs (K3s/Flux pattern). `deploy/enzarb/enzarb.yaml` deploys the main chart.

```bash
mise run deploy-system   # kubectl apply -f deploy/system/
mise run deploy-enzarb   # kubectl apply -f deploy/enzarb/
mise run status          # kubectl get helmchart -A && kubectl get project,environment -A
mise run logs-operator   # stream operator logs
mise run admin           # kubectl exec into operator for admin CLI
```

## CRD generation

CRD YAML files are in `operator/config/crd/`. Regenerate with kubebuilder:
```bash
cd operator && go generate ./...
```
(kubebuilder markers are on types in `operator/api/v1alpha1/types.go`)
