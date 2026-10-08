# Database: shared Postgres via pgop

Postgres is provisioned with [pgop](https://pgop.ruck.io) (`pgop.ruck.io/v1alpha1`: `Cluster`, `Role`, `Database`). See [ADR-0009](adr/0009-database-sharing.md).

## Layout

**One `Cluster` (`enzarb-pg`), one `Database` (`enzarb`), one schema per owning repo, one role per service.**
- No service reads or writes another repo's tables.
- Each repo that shares data publishes a small, versioned **API schema** of views and functions. That is the only cross-repo contract.

| Schema | Owner (migrates it) | Contents | Who else gets access |
|---|---|---|---|
| `workflows` | enzarb-workflows | `events`, `work_items`, `transitions`, `jobs` | nobody |
| `workflows_api` | enzarb-workflows | views `work_item_v`, `transition_v`, `job_v`; functions `ingest_event(...)`, `submit_action(...)` (both `SECURITY DEFINER`) | ui: `SELECT` views, `EXECUTE submit_action`; connectors: `EXECUTE ingest_event` |
| `connectors` | enzarb-connectors | installations, connector accounts, polling cursors, cached tokens (encrypted) | workflows executor, through the connectors role |
| `app` | enzarb-ui | users, sessions, linked identities (enzarb user ↔ GitHub login), preferences | nobody |
| `app_api` | enzarb-ui | view `identity_v` | workflows: `SELECT` (authorizing triggers) |

Functions:
- `workflows_api.ingest_event(source, dedupe_key, work_item_key, type, payload)` inserts into `workflows.events` with `ON CONFLICT DO NOTHING`, then runs `NOTIFY workflow_events`.
- `workflows_api.submit_action(work_item_id, actor, action, payload)` lets the UI start, approve, cancel or retry. It is ingested as a `ui:` event.

Projects are **not** in Postgres. They stay as Kubernetes `Project` CRs (low churn, owned by the operator). Workflows and connectors build an in-memory index of repo remote → Project by watching the CRs.

## pgop resources (namespace `enzarb`, rendered by `enzarb-helm`)

```yaml
apiVersion: pgop.ruck.io/v1alpha1
kind: Cluster
metadata: { name: enzarb-pg }
spec:
  image: postgres:18
  storage: { size: 20Gi, retainPolicy: Retain }
---
# Migration roles: one per owning repo, used only by that repo's migration Job
apiVersion: pgop.ruck.io/v1alpha1
kind: Role
metadata: { name: workflows-owner }
spec:
  clusterRef: { name: enzarb-pg }
  roleName: enzarb_workflows_owner
  login: true
---
# Runtime roles: data read/write only
apiVersion: pgop.ruck.io/v1alpha1
kind: Role
metadata: { name: workflows }
spec:
  clusterRef: { name: enzarb-pg }
  roleName: enzarb_workflows
  login: true
# …same pattern for connectors-owner/connectors and ui-owner/ui
---
apiVersion: pgop.ruck.io/v1alpha1
kind: Database
metadata: { name: enzarb }
spec:
  clusterRef: { name: enzarb-pg }
  databaseName: enzarb
  owner: workflows-owner
  schemas:
    - { name: workflows,     owner: workflows-owner }
    - { name: workflows_api, owner: workflows-owner }
    - { name: connectors,    owner: connectors-owner }
    - { name: app,           owner: ui-owner }
    - { name: app_api,       owner: ui-owner }
```

The `Database`'s field shapes (schema grants in particular) must be checked against the pgop API reference before this goes into the chart.

### Who grants what
- **Basic access:** `Database.spec.schemas` creates each schema with its owner and `USAGE` grants.
- **Object-level access:** grants on specific views and functions, plus `ALTER DEFAULT PRIVILEGES` for the runtime role on its own schema, are written in **each owner's migrations**. They use pgop's deterministic role names (`enzarb_ui`, `enzarb_connectors`, `enzarb_workflows`).
- **Separate history:** each repo uses its own migrations table inside its schema (`workflows._sqlx_migrations`, `app._migrations`, …).

### Credentials
pgop writes the Secret `<cluster>-<role>-credentials`, for example `enzarb-pg-workflows-credentials`. Its keys are `username`, `password`, `host`, `port`, `sslmode`, `uri` and `ca.crt` (with TLS). The name is deterministic, so Helm wires `DATABASE_URL` from key `uri` at template time, before pgop's status is filled in.

### Migrations
Each service's chart includes a pre-install/pre-upgrade Job that runs its migrations with the **owner** role's Secret. The runtime Deployments use the runtime role only.

## Live updates to the UI
- The reducer runs `NOTIFY work_item_changed, '<id>'`.
- `enzarb-ui` keeps **one** direct LISTEN connection per pod and fans updates out to browsers over SSE.
- Remote-function queries then refresh from the `workflows_api` views.
- The UI keeps no copy of workflow state.

## Alternatives considered
- **One schema shared by everyone:** every migration becomes a cross-repo breaking change.
- **A database per service plus an HTTP/gRPC API in workflows:** adds a service, loses LISTEN/NOTIFY and the single-step insert-and-notify, and turns UI remote functions into API clients. Still possible later, because `workflows_api` already is the contract.

## Known pgop gaps for this use
- **No failover:** `Cluster.spec.replicas` is max 1.
- **Undocumented:** backups, PITR and connection pooling aren't in the documented API. Postgres is now the source of truth for the workflow engine, so these are needed before production, or a recovery point objective must be explicitly accepted.
- **Pooling:** if a pooler is added, LISTEN connections must not use transaction mode.
- **Grants:** need to confirm whether `Database.grants` covers object-level and default privileges, or migrations keep handling them.
