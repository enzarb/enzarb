# enzarb-helm

Umbrella chart for enzarb (`enzarb/`). Contracts: `docs/contracts.md` and `docs/database.md` in the top-level `enzarb` repo.

## Prerequisites (enzarb-kubernetes)
Gateway `enzarb` in `enzarb-system` (TLS secret `enzarb-tls`), agent-sandbox, and pgop v0.9.1 CRDs/operator.

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
| workflows | Deployment (`run --roles ...`), Service `enzarb-workflows-executor` :8081, ClusterRole projects + Role `secrets` get in the sandbox namespace |
| connectors | Deployment `receiver serve`, Service :8080 |
| realtime (`realtime.enabled`, default on) | Deployment (2 replicas, stateless), Service :8080, ServiceAccount (no RBAC), pgop Role `realtime` (`enzarb_realtime`), HTTPRoute `/live/` (request timeout `route.realtimeRequestTimeout`, default `0s`, for websockets). `ENZARB_REALTIME_JWT_SECRET` is one generated key (`realtime-jwt-secret` in `enzarb-generated`, or `secrets.realtimeJwt.existingSecret`) mounted into realtime and the ui. Grants on `work_item_project_v` are applied by enzarb-workflows, not the chart: it needs enzarb-workflows >= v0.12.0 (the release containing the grant reconciler, enzarb-workflows#62), which runs at startup and from its ticker (5s), so grants land within one tick of the role appearing. `realtime.allowedOrigins` (list, default empty = `https://<host>`, or `publicUrl` reduced to scheme+host[:port]) is rendered as the comma-separated `ENZARB_REALTIME_ALLOWED_ORIGINS` Origin allowlist for cookie-authenticated websockets; entries must be bare origins (`scheme://host[:port]`, no path, trailing `/`, userinfo, query or fragment); the schema and a template guard (also applied to the derived default) reject anything else, matching what enzarb-realtime accepts at startup. `realtime.terminationGracePeriodSeconds` (default 30, minimum 10). With `route.backendTrafficPolicy.enabled`, the idle-timeout BackendTrafficPolicy also covers `/live/` (policy `enzarb-realtime`). Probes: `/readyz` (200 only while LISTEN is up), `/healthz`. Envoy access logs, if configured, must redact the `token` query parameter (log `%REQ_WITHOUT_QUERY(:PATH)%`). |
| sandbox-proxy | Deployment, Service :8080, ClusterRole projects + Role `secrets` get in the sandbox namespace |
| operator | Deployment (2 replicas, leader election), ClusterRole (Projects, Project finalizers) + Role in the sandbox namespace (secrets, PVCs, NetworkPolicies, Sandboxes, events), leader-election Role; optional ConfigMap `enzarb-sandbox-operator-docker-profiles` when `operator.docker.profiles` is set |
| sandbox namespace | Namespace `sandbox.namespace` (`enzarb-sandboxes`, kept on uninstall), ServiceAccount `enzarb-sandbox` (no token, no RBAC), LimitRange, default-deny NetworkPolicy (ADR-0013) |
| pgop | Cluster `enzarb-pg`, 6 Roles (`<svc>-owner`, `<svc>`; plus `realtime` when enabled), Database `enzarb` with 5 schemas |
| migrations | 3 Jobs: workflows, connectors, ui |
| routes | HTTPRoutes `/` `/sandbox/` `/hooks/` on `host` | The proxy idle timeout (`sandboxProxy.idleTimeoutSecs`, default 2100) must stay at or above the chat idle (1800); with `route.backendTrafficPolicy.enabled`, its `connectionIdleTimeout` (default 2400s) must exceed the proxy idle, or Envoy cuts idle chat websockets first. The route request timeout is `0s`. The proxy pings the browser every 30 s.

## Shared sandbox namespace (ADR-0013)
All Projects land in one namespace, `sandbox.namespace` (`ENZARB_SANDBOX_NAMESPACE`, default `enzarb-sandboxes`). The chart creates it with `helm.sh/resource-policy: keep` (it holds Project volumes), Pod Security `enforce: privileged` (the dockerd sidecar needs SYS_ADMIN/NET_ADMIN under gVisor, ADR-0012), a default-deny NetworkPolicy (the operator adds per-Project allow policies) and a LimitRange (`sandbox.limitRange`). Sandbox pods run as `sandbox.serviceAccount` (`ENZARB_SANDBOX_SERVICE_ACCOUNT`, default `enzarb-sandbox`) with no token and no RBAC: sandboxes have no Kubernetes API access. **`sandbox.egress.clusterCIDRs` is required** (pod, service and node networks; rendering fails without it) -> `ENZARB_SANDBOX_EGRESS_EXCLUDE_CIDRS`; optional `sandbox.dnsCIDRs` -> `ENZARB_SANDBOX_DNS_CIDRS`. `sandbox.executorCallbackPort` -> `ENZARB_EXECUTOR_CALLBACK_PORT`. The operator, workflows executor and sandbox-proxy get namespaced Roles there; Capsule is no longer used.

## Sandbox runtime and Docker (ADR-0012)
Docker is always on and the runtime class is cluster-admin config, not per Project.
- `operator.sandboxRuntimeClass` (default `gvisor`; schema allows `gvisor`, `kata`) -> `ENZARB_SANDBOX_RUNTIME_CLASS`. It must be listed in `operator.docker.runtimeClasses` (default `[gvisor]`, -> `ENZARB_DOCKER_RUNTIME_CLASSES`) or the operator refuses to start.
- `operator.docker.image` (`docker:29.8.2-dind`), `operator.docker.storage` (`50Gi`, size of each Project's `docker` PVC), `operator.docker.resources.{requests,limits}.{cpu,memory}` (dockerd sidecar; unset keys are omitted).
- `operator.docker.profiles`: extra `{<runtimeClass>: {securityContext, dockerdArgs}}` profiles. When non-empty the chart renders a ConfigMap, mounts it at `/etc/enzarb/docker` and sets `ENZARB_DOCKER_PROFILES_FILE`.

Every Project now has two PVCs: `workspace` and `docker`. Sandbox nodes must carry the label `enzarb.dev/runtime-gvisor=true` (installed by enzarb-kubernetes together with the `gvisor` RuntimeClass); without a matching node, sandbox pods stay Pending.

## Private npm packages for agent runs

`workflows.npmToken` (default enabled) wires `ENZARB_NPM_TOKEN` on the workflows Deployment (executor role) from Secret `enzarb-updater-registry`, key `registry-token` (`existingSecret` / `key` override; the Secret must be in the release namespace). The reference is `optional: true`: if the Secret or key is missing the variable stays unset and the pod still starts, but agent `npm ci` of private `@enzarb/*` packages then fails with E403. The value is the long-lived GHCR read PAT (`read:packages` covers every package its owner can read); enzarb-workflows passes it to sandbox-init as `RunStart.npm_token`, which exposes it to the agent process only as `NODE_AUTH_TOKEN`, redacted and never persisted. Set `workflows.npmToken.enabled=false` to omit it. Needs an enzarb-workflows release with `ENZARB_NPM_TOKEN` support and an enzarb-sandbox-container release with `npm_token` support.

## Migration ordering
Jobs are plain resources, not Helm hooks (hooks would deadlock `helm install --wait`: pods would need schemas that only post-install hooks create). Init containers (`kubectl wait`, image `waitImage`) enforce the order: workflows waits for the pgop Cluster, Database and all Roles (`migrate --require-roles` needs the runtime roles), connectors waits for the workflows Job, ui for the connectors Job, and every runtime Deployment (ui, workflows, connectors) for the ui Job. Job names embed a hash of the Job's inputs (image, pull policy, DB names, wait image, security context, upstream Job name, a template revision), so any change creates a new Job and unchanged inputs keep the completed one.

## Database credentials
pgop creates `<cluster>-<role>-credentials` per Role (keys `username password host port sslmode`, `uri` pointing at `postgres`) and a per-database Secret only for the Database owner. The chart passes `PGHOST PGPORT PGUSER PGPASSWORD PGSSLMODE` from the Role Secret plus `PGDATABASE` from values, and sets `DATABASE_URL=postgres:///enzarb` (no credentials, so passwords with URL-special characters are safe). sqlx (workflows, connectors) falls back to the `PG*` variables. postgres.js (ui) honors all of them except `PGSSLMODE`, so the ui gets `DATABASE_URL=postgres:///enzarb?sslmode=$(PGSSLMODE)`. ui probes are HTTP (`/healthz`, `/readyz`); set `ui.probes.http=false` for TCP.

## Uninstall keeps data definitions
The pgop `Cluster`, `Database` and `Role` CRs and the generated Secret (`enzarb.generatedSecret`: session JWT, executor callback secret, realtime JWT) carry `helm.sh/resource-policy: keep`, so `helm uninstall` (including the helm-controller's reinstall failure policy) cannot delete the database definition or the secrets. The PVC is created by pgop, not Helm, and is untouched. Nothing else is kept, so normal upgrades are unaffected.

Consequence: a later `helm install` finds these objects already present and fails with "exists and cannot be imported". Either adopt them first or let Helm take ownership:
```sh
helm install enzarb <chart> -n enzarb --take-ownership   # Helm 3.17+/4
# or, per kept object (Cluster, Database, Roles, Secret):
kubectl -n enzarb annotate <kind>/<name> meta.helm.sh/release-name=enzarb meta.helm.sh/release-namespace=enzarb --overwrite
kubectl -n enzarb label <kind>/<name> app.kubernetes.io/managed-by=Helm --overwrite
```
The generated Secret is read back with `lookup`, so adopted values are preserved. Under the RKE2 helm-controller use the annotation route (unverified: whether the controller exposes `--take-ownership`). To really delete the data, remove the kept objects by hand.

## Tests
`mise install`, then `mise run test`: `helm lint`, golden snapshots (`scripts/golden.sh`, `--update` to rewrite `tests/golden/`), kubeconform (CRD schemas from the datree catalog), and `scripts/check-pgop.py` (pgop resources against the vendored CRDs).

## Component versions and releases
Every component image tag in `enzarb/values.yaml` (and `operator.sandboxImage`) is a pinned release version, never `main`. They are written by `scripts/sync-components.sh`, which reconciles each entry of `scripts/components.yaml` to its repo's latest GitHub release, but only when that release tag's `ci` run succeeded (its docker job pushes `ghcr.io/enzarb/<image>:<version>`) and the version is newer than the current pin; any other GitHub API failure fails the run. The `sync-components` workflow runs it when a component dispatches `component-released` (and every 6 hours as a safety net), refreshes the goldens, runs the checks and commits `feat(deps)`/`fix(deps)` to `main`; the release workflow then tags the next chart version and `publish.yml` pushes `oci://ghcr.io/enzarb/charts/enzarb`. `Chart.yaml` keeps `0.0.0` placeholders. See `docs/ci.md` in the workspace repo.

`enzarb/values-oldie.yaml` is the single-node production install (`enzarb.dev`, StorageClass `enzarb-lvm`, `IfNotPresent`); render it with `scripts/render.sh oldie`.
