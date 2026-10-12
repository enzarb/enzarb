# enzarb-workflows

The enzarb workflow engine on Postgres: migrations, state machines, reducer, executors, ticker.

See [docs/architecture.md](https://github.com/enzarb/enzarb/blob/main/docs/architecture.md) in the top-level `enzarb` repo.

## Binary

```
enzarb-workflows migrate [--require-roles]
enzarb-workflows run --roles reducer,executor,ticker      # or ENZARB_ROLES
```

- `migrate` applies the embedded migrations (run it as the schema owner role) and exits.
  `--require-roles` fails, applying nothing, unless `enzarb_connectors`, `enzarb_ui` and
  `enzarb_workflows` exist. Without it, grants to missing roles are skipped with a NOTICE.
  Use `--require-roles` in production. `enzarb_realtime` is optional: it is not in `--require-roles`.
  Its grants are `CONNECT` on the database, `USAGE` on `workflows_api` and `SELECT` on
  `workflows_api.work_item_project_v`, nothing else. `migrate` applies them when the role exists,
  and the ticker role reconciles them idempotently at startup and every pass (via the
  `SECURITY DEFINER` function `workflows.reconcile_realtime_grants()`), so a role created later
  by pgop gets them too. Run `migrate` before upgrading `run`; until then the ticker logs one warning.
- `run` starts the listed roles in one process. All share one shutdown (SIGTERM/SIGINT); if
  any role exits or fails, the process shuts the others down and exits non-zero on error.
  Replicas of any role can run concurrently (work is claimed in Postgres).

### Environment

| Var | Roles | Default |
|---|---|---|
| `DATABASE_URL` | all, `migrate` | required |
| `ENZARB_ROLES` | `run` (alternative to `--roles`) | |
| `ENZARB_DB_MAX_CONNECTIONS` | all | 10 |
| `ENZARB_BOT_LOGIN`, `ENZARB_BOT_HANDLE` | reducer, executor | `enzarb[bot]`, `enzarb` |
| `GITHUB_APP_ID`, `GITHUB_APP_PRIVATE_KEY` | reducer, executor | required for those roles |
| `ENZARB_EXECUTOR_URL` | executor (callback base) | `http://enzarb-workflows-executor.enzarb.svc:8081` |
| `ENZARB_EXECUTOR_CALLBACK_SECRET` | executor | required; shared by all replicas |
| `ENZARB_EXECUTOR_LISTEN` | executor | `0.0.0.0:8081` |
| `ENZARB_EXECUTOR_WORKERS` | executor | 24 (agent jobs hold a worker only while being accepted, not while the run is active) |
| `ENZARB_EXECUTOR_MAX_AGENT_PER_PROJECT` | executor | 16 (ceiling for `Project.spec.maxRuns`) |
| `ENZARB_EXECUTOR_DEFAULT_AGENT_PER_PROJECT` | executor | 2 (Projects without `spec.maxRuns`) |
| `ENZARB_EXECUTOR_SANDBOX_CHANGE_HOLD_CEILING_SECS` | executor | 10800 (a `status.pendingChange` hold older than this stops holding agent jobs, with a warning) |
| `ENZARB_HEALTH_LISTEN` | processes without `executor` | `0.0.0.0:8082` |
| `RUST_LOG` | all | `info` (logs are JSON on stdout) |

Reducer and executor roles need a Kubernetes client (in-cluster or `KUBECONFIG`).

### Ports

- `:8081` (executor role): `POST /v1/runs/{run_id}/finished` (sandbox-init callback) and `GET /healthz`.
- `:8082` (no executor role): `GET /healthz`.

### RBAC

`deploy/rbac.yaml`: `get/list/watch` and `patch` (the `enzarb.dev/drained-generation` annotation, set once a Project's sandbox has drained for a pending change) on `projects.enzarb.dev` cluster-wide (ClusterRole: reducer index,
executor sandbox lookup) and, in the shared sandbox namespace (`enzarb-sandboxes`, ADR-0013), a namespaced
Role with `get` on `secrets` (the per-Project auth Secrets `sb-<project>-auth`; names vary per Project, so
`resourceNames` cannot pin them, and the namespace holds no other Secrets). Only the executor role needs the
Secret access. The executor finds a sandbox only through `Project.status.namespace` and `status.sandbox`;
a Project without them is not ready and the lookup is retried (`SandboxError::NotReady`).

## Image

The workspace has private git dependencies (enzarb-proto, enzarb-sandbox-operator,
enzarb-connectors). Each needs its own read-only deploy key, passed as a build secret
(`--ssh default` breaks with several keys; see "Docker builds with private dependencies" in
the enzarb `docs/contracts.md`):

```
K=$HOME/.ssh/id_ed25519   # any key with access to all three repos
docker build --secret id=proto_key,src=$K --secret id=crds_key,src=$K \
  --secret id=connectors_key,src=$K -t enzarb-workflows .
```

The image runs as uid 65532 and defaults to `run --roles reducer,executor,ticker`.

## Development

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace     # integration tests start Postgres via testcontainers (Docker)
```
