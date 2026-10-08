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
| operator | Deployment (2 replicas, leader election), ClusterRole/Role from its `deploy/rbac.yaml` |
| pgop | Cluster `enzarb-pg`, 6 Roles (`<svc>-owner`, `<svc>`), Database `enzarb` with 5 schemas |
| migrations | 3 Jobs: workflows, connectors, ui |
| routes | HTTPRoutes `/` `/sandbox/` `/hooks/` on `host` |

## Migration ordering
Jobs are plain resources, not Helm hooks (hooks would deadlock `helm install --wait`: pods would need schemas that only post-install hooks create). Init containers (`kubectl wait`, image `waitImage`) enforce the order: workflows waits for the pgop Cluster, Database and all Roles (`migrate --require-roles` needs the runtime roles), connectors waits for the workflows Job, ui for the connectors Job, and every runtime Deployment (ui, workflows, connectors) for the ui Job. Job names embed a hash of the image, so a new image creates a new Job and an unchanged image keeps its completed one.

## Database credentials
pgop creates `<cluster>-<role>-credentials` per Role (keys `username password host port sslmode`, `uri` pointing at `postgres`) and a per-database Secret only for the Database owner. The chart therefore builds `DATABASE_URL` from the Role Secret keys with `$(VAR)` expansion and the database name from values.

## Tests
`mise install`, then `mise run test`: `helm lint`, golden snapshots (`scripts/golden.sh`, `--update` to rewrite `tests/golden/`), kubeconform (CRD schemas from the datree catalog), and `scripts/check-pgop.py` (pgop resources against the vendored CRDs).
