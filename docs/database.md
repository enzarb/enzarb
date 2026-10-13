# Database: shared Postgres via pgop

Postgres is provisioned with [pgop](https://pgop.ruck.io) (`pgop.ruck.io/v1alpha1`: `Cluster`, `Role`, `Database`, plus `Backup`, `BackupRun` and `Restore` as of v0.9.1). See [ADR-0009](adr/0009-database-sharing.md).

## Layout

**One `Cluster` (`enzarb-pg`), one `Database` (`enzarb`), one schema per owning repo, one role per service.**
- No service reads or writes another repo's tables.
- Each repo that shares data publishes a small, versioned **API schema** of views and functions. That is the only cross-repo contract.

| Schema | Owner (migrates it) | Contents | Who else gets access |
|---|---|---|---|
| `workflows` | enzarb-workflows | `events`, `work_items`, `transitions`, `jobs` | nobody |
| `workflows_api` | enzarb-workflows | views `work_item_v`, `transition_v`, `job_v`; functions `ingest_event(...)`, `submit_action(...)` (both `SECURITY DEFINER`) | ui: `SELECT` views, `EXECUTE submit_action`; connectors: `EXECUTE ingest_event` |
| `connectors` | enzarb-connectors | installations, connector accounts, polling cursors, cached tokens (encrypted) | workflows executor, through the connectors role |
| `app` | enzarb-ui | users, sessions, linked identities (enzarb user ↔ GitHub login), preferences, `claude_credentials` (one row per user, encrypted Claude token; the UI's runtime role has **no** grant on it) | nobody |
| `app_api` | enzarb-ui | views `identity_v`, `claude_credential_v`; `SECURITY DEFINER` functions `claude_credential_put/delete/get/list_stale/rekey` | workflows: `SELECT identity_v` (authorizing triggers), `EXECUTE claude_credential_get`; ui: `SELECT claude_credential_v`; sandbox-proxy: `EXECUTE claude_credential_put/delete/get/list_stale/rekey` |

Functions:
- `workflows_api.ingest_event(source, dedupe_key, work_item_key, type, payload)` inserts into `workflows.events` with `ON CONFLICT DO NOTHING`, then runs `NOTIFY workflow_events`.
- `workflows_api.submit_action(work_item_id, actor, action, payload)` lets the UI start, approve, cancel, retry or merge. It is ingested as a `ui:` event.

**Claude token ([ADR-0004](adr/0004-claude-credentials.md) amendment 2026-10-13).** `app.claude_credentials(owner text primary key, key_id text not null, ciphertext bytea not null, created_at, updated_at)`; `owner` is the GitHub login (`spec.owner`).
- **Encryption:** AEAD (AES-256-GCM, random 96-bit nonce stored with the ciphertext), with `owner` and `key_id` as associated data, so a row cannot be moved to another user or key. The keys come from the Secret `enzarb-claude-key` in namespace `enzarb`: a keyring whose data keys are `<key_id>` (value: 32 random bytes) plus one key `active` whose value is the id of the key used for new writes. Decryption picks the key by the row's `key_id`; a missing id fails closed. It is mounted **only** into sandbox-proxy and the workflows executor (the services that decrypt). It is never in `enzarb-sandboxes` and never reaches the UI, operator, connectors or a sandbox.
- **Byte layout (shared by sandbox-proxy and workflows):** a key file holds the 32 raw key bytes. `ciphertext` = 12-byte nonce ‖ AES-256-GCM output (ciphertext ‖ 16-byte tag). Associated data = `"enzarb-claude-v1"` ‖ `0x00` ‖ owner (UTF-8) ‖ `0x00` ‖ key_id (UTF-8). Both services must pass this test vector: key = bytes `00..1f`, nonce = bytes `00..0b`, owner `octocat`, key_id `k1`, plaintext `sk-ant-oat01-test` → `ciphertext` hex `000102030405060708090a0b3469fb7aab91ef74ec35a7ba9c9d1d1ef7f96b381458aac61771c67de398262ffd`.
- **Key rotation:** add the new key to the keyring and mark it active (old keys stay listed), then roll sandbox-proxy and workflows. Run `enzarb-sandbox-proxy rekey` (a chart Job or `mise` task): it lists rows whose `key_id` is not the active one (`claude_credential_list_stale`), decrypts each with its old key and calls `claude_credential_rekey`, one row at a time. A row changed in between is skipped and retried. Remove the old key from the keyring once the Job reports no stale rows. New writes always use the active key.
- **Roles (ADR-0009):** the ciphertext is reachable only through the functions. `enzarb_sandbox_proxy` (a new runtime role: pgop Role plus credentials Secret, like the others) can write, delete, read (for chat opens), list stale rows and re-key. `enzarb_workflows` can read (`claude_credential_get`, per run). `enzarb_ui` can `SELECT` only `app_api.claude_credential_v(owner, present, updated_at)`, which has no `key_id` or `ciphertext` column, and has no privilege on `app.claude_credentials` (the UI migration revokes the schema's default privileges on that table). Connectors and the operator have no access. The owner role `enzarb_ui_owner` migrates the table but holds no key, so it cannot decrypt.
- **Functions** (`SECURITY DEFINER`, `search_path` pinned): `claude_credential_put(owner, key_id, ciphertext, only_if_absent bool)` upserts, or inserts only when absent (legacy migration), and returns whether a row now exists; `claude_credential_delete(owner)`; `claude_credential_get(owner)` returns `(key_id, ciphertext)`; `claude_credential_list_stale(active_key_id)` returns `(owner, key_id)`; `claude_credential_rekey(owner, old_key_id, new_key_id, ciphertext)` updates only if the row still has `old_key_id`.

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
Verified against pgop v0.9.1 (enzarb-helm#2):
- **Role secrets:** each `Role` gets `<cluster>-<role>-credentials` (e.g. `enzarb-pg-ui-credentials`) with keys `username, password, host, port, sslmode, uri` (+`ca.crt` with TLS). Its `uri` targets the `postgres` database.
- **Database secret:** a Database secret `<database>-<owner>-credentials` exists only for the Database owner.
- **What the chart does:** passes `PGHOST, PGPORT, PGUSER, PGPASSWORD, PGSSLMODE` from each service's Role secret plus `PGDATABASE=enzarb`, and sets `DATABASE_URL=postgres:///enzarb`, so passwords never go into a URL. sqlx (workflows, connectors) and postgres.js (ui) fill the missing parts from `PG*`. postgres.js ignores `PGSSLMODE`, so the ui gets `postgres:///enzarb?sslmode=$(PGSSLMODE)` (enzarb-helm#4).
- **Role names:** `schemas[].owner` and `schemas[].grants[].role` take raw PostgreSQL role names (e.g. `enzarb_ui_owner`). `Database.owner` takes a Role resource name.
- **Grants:** there is no `Database.grants`; use `schemas[].grants`. Its keys are `username`, `password`, `host`, `port`, `sslmode`, `uri` and `ca.crt` (with TLS). The name is deterministic, so Helm wires `DATABASE_URL` from key `uri` at template time, before pgop's status is filled in.

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
