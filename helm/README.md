# enzarb-helm

Umbrella chart for enzarb (`enzarb/`). Contracts: `docs/contracts.md` and `docs/database.md` in the top-level `enzarb` repo.

## Prerequisites (enzarb-kubernetes)
Gateway `enzarb` in `enzarb-system` (TLS secret `enzarb-tls`), Capsule (its `userGroups` must include `system:serviceaccounts:<release namespace>`), agent-sandbox, and pgop v0.9.1 CRDs/operator.

## Install
```sh
helm install enzarb enzarb -n enzarb --create-namespace \
  --set secrets.github.existingSecret=enzarb-github          # prod
helm install enzarb enzarb -n enzarb --create-namespace -f enzarb/values-dev.yaml   # kind (kind load the :dev images)
```
`crds/project.yaml` is installed on first install only (Helm never upgrades `crds/`). Refresh it with `scripts/sync-crds.sh` and apply it manually on upgrades.

## What it renders
| Component | Resources |
|---|---|
| ui | Deployment, Service :3000, ClusterRole `projects` get/list/create/delete |
| workflows | Deployment (`run --roles ...`), Service `enzarb-workflows-executor` :8081, ClusterRole projects + `secrets/sandbox-auth` get |
| connectors | Deployment `receiver serve`, Service :8080 |
| sandbox-proxy | Deployment, Service :8080, ClusterRole projects + `secrets/sandbox-auth` get |
| operator | Deployment (2 replicas, leader election), ClusterRole/Role from its `deploy/rbac.yaml`; optional ConfigMap `enzarb-sandbox-operator-docker-profiles` when `operator.docker.profiles` is set |
| pgop | Cluster `enzarb-pg`, 6 Roles (`<svc>-owner`, `<svc>`), Database `enzarb` with 5 schemas |
| migrations | 3 Jobs: workflows, connectors, ui |
| routes | HTTPRoutes `/` `/sandbox/` `/hooks/` on `host` |

## Sandbox runtime and Docker (ADR-0012)
Docker is always on and the runtime class is cluster-admin config, not per Project.
- `operator.sandboxRuntimeClass` (default `gvisor`; schema allows `gvisor`, `kata`) -> `ENZARB_SANDBOX_RUNTIME_CLASS`. It must be listed in `operator.docker.runtimeClasses` (default `[gvisor]`, -> `ENZARB_DOCKER_RUNTIME_CLASSES`) or the operator refuses to start.
- `operator.docker.image` (`docker:29.8.2-dind`), `operator.docker.storage` (`50Gi`, size of each Project's `docker` PVC), `operator.docker.resources.{requests,limits}.{cpu,memory}` (dockerd sidecar; unset keys are omitted).
- `operator.docker.profiles`: extra `{<runtimeClass>: {securityContext, dockerdArgs}}` profiles. When non-empty the chart renders a ConfigMap, mounts it at `/etc/enzarb/docker` and sets `ENZARB_DOCKER_PROFILES_FILE`.

Every Project now has two PVCs: `workspace` and `docker`. Sandbox nodes must carry the label `enzarb.dev/runtime-gvisor=true` (installed by enzarb-kubernetes together with the `gvisor` RuntimeClass); without a matching node, sandbox pods stay Pending.

## Migration ordering
Jobs are plain resources, not Helm hooks (hooks would deadlock `helm install --wait`: pods would need schemas that only post-install hooks create). Init containers (`kubectl wait`, image `waitImage`) enforce the order: workflows waits for the pgop Cluster, Database and all Roles (`migrate --require-roles` needs the runtime roles), connectors waits for the workflows Job, ui for the connectors Job, and every runtime Deployment (ui, workflows, connectors) for the ui Job. Job names embed a hash of the Job's inputs (image, pull policy, DB names, wait image, security context, upstream Job name, a template revision), so any change creates a new Job and unchanged inputs keep the completed one.

## Database credentials
pgop creates `<cluster>-<role>-credentials` per Role (keys `username password host port sslmode`, `uri` pointing at `postgres`) and a per-database Secret only for the Database owner. The chart passes `PGHOST PGPORT PGUSER PGPASSWORD PGSSLMODE` from the Role Secret plus `PGDATABASE` from values, and sets `DATABASE_URL=postgres:///enzarb` (no credentials, so passwords with URL-special characters are safe). sqlx (workflows, connectors) falls back to the `PG*` variables. postgres.js (ui) honors all of them except `PGSSLMODE`, so the ui gets `DATABASE_URL=postgres:///enzarb?sslmode=$(PGSSLMODE)`. ui probes are HTTP (`/healthz`, `/readyz`); set `ui.probes.http=false` for TCP.

## Tests
`mise install`, then `mise run test`: `helm lint`, golden snapshots (`scripts/golden.sh`, `--update` to rewrite `tests/golden/`), kubeconform (CRD schemas from the datree catalog), and `scripts/check-pgop.py` (pgop resources against the vendored CRDs).
