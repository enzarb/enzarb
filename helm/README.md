# enzarb-helm

Umbrella chart for enzarb (`enzarb/`). Contracts: `docs/contracts.md` and `docs/database.md` in the top-level `enzarb` repo.

## Prerequisites (`kubernetes/`)
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

Every Project now has two PVCs: `workspace` and `docker`. Sandbox nodes must carry the label `enzarb.dev/runtime-gvisor=true` (installed by `kubernetes/` together with the `gvisor` RuntimeClass); without a matching node, sandbox pods stay Pending.

## Private npm packages for agent runs

`workflows.npmToken` (default enabled) wires `ENZARB_NPM_TOKEN` on the workflows Deployment (executor role) from Secret `enzarb-updater-registry`, key `registry-token` (`existingSecret` / `key` override; the Secret must be in the release namespace). The reference is `optional: true`: if the Secret or key is missing the variable stays unset and the pod still starts, but agent `npm ci` of private `@enzarb/*` packages then fails with E403. The value is the long-lived GHCR read PAT (`read:packages` covers every package its owner can read); enzarb-workflows passes it to sandbox-init as `RunStart.npm_token`, which exposes it to the agent process only as `NODE_AUTH_TOKEN`, redacted and never persisted. Set `workflows.npmToken.enabled=false` to omit it. Needs an enzarb-workflows release with `ENZARB_NPM_TOKEN` support and an enzarb-sandbox-container release with `npm_token` support.

## Claude token keyring
Per-user Claude tokens are stored encrypted. The keyring is Secret `enzarb-claude-key` in the release namespace: one data key `<key_id>` per key (32 random bytes) plus `active`, the id new tokens are encrypted with. It is generated once (read back with `lookup` on upgrade, `helm.sh/resource-policy: keep`) unless `secrets.claudeKey.existingSecret` names your own. It is mounted read-only at `/etc/enzarb/claude-key` (`ENZARB_CLAUDE_KEYRING_DIR`) into sandbox-proxy and into the workflows Deployment when the `executor` role is enabled, and never rendered into or mounted in the sandbox namespace. Because reducer, executor and ticker share one Deployment, the reducer and ticker pods also see it. sandbox-proxy gets its own pgop Role `enzarb_sandbox_proxy` (credentials `<cluster>-sandbox-proxy-credentials`, `USAGE` on `app_api`); its object grants come from the ui migration.

**Back the Secret up.** Losing it makes every stored token undecryptable. `helm template` / `--dry-run` cannot see the live Secret and show a fresh key; real installs and upgrades reuse it.

Rotation: add a new `<key_id>` entry to the Secret and point `active` at it (keep the old ids), roll sandbox-proxy and workflows, then run `mise run rekey` (`ENZARB_CHART` selects the chart, default the published one; it reuses the release values). That renders `secrets.claudeKey.rekey.enabled=true` with a fresh `rekey.suffix` and applies one Job running the sandbox-proxy image with args `[rekey]`, which re-encrypts stale rows. The Job has no Kubernetes API access and never edits the Secret. It is not a hook and never runs on upgrade. Needs sandbox-proxy and ui releases that implement the keyring and `claude_credential_*` functions.

## Web Push keypair
UI push notifications are signed with a VAPID keypair in Secret `enzarb-vapid-key` (data keys `public-key` and `private-key`, base64url) in the release namespace. Sprig cannot generate P-256 keys, so a `pre-install,pre-upgrade` hook Job (`enzarb-vapid-key`, the UI image running `scripts/vapid-secret.ts`) creates the Secret when it does not exist and never touches an existing one (browser subscriptions are bound to the public key; the Secret carries `helm.sh/resource-policy: keep`). Its ServiceAccount (`enzarb-vapid`, also a hook) may only create Secrets and read `enzarb-vapid-key` in the release namespace. `secrets.vapidKey.existingSecret` names your own Secret (same key names) and renders no Job. `secrets.vapidKey.subject` (`mailto:` or https URL, default the public URL) is the VAPID `sub` claim.

Only the `enzarb-ui` Deployment reads the Secret (`ENZARB_VAPID_PUBLIC_KEY`, `ENZARB_VAPID_PRIVATE_KEY`, both optional references). If the Secret is missing, or the UI image predates `scripts/vapid-secret.ts` (the Job then logs that and succeeds), pods still start and notifications stay off. **Back the Secret up**, or every device has to enable notifications again after it is lost. `mise run vapidkey` asserts the wiring.

## Zero-downtime rollouts
Every Deployment (ui, workflows, connectors, sandbox-proxy, realtime, operator, updater) renders `strategy: RollingUpdate` with `<component>.rollout.maxSurge` (default 1) and `<component>.rollout.maxUnavailable` (default 0): the new pod must be Ready before the old one is stopped. Readiness probes poll every 2s so the new pod joins quickly. On shutdown, `<component>.preStopSeconds` (default 10; updater 5; operator 0 because it receives no traffic; 0 omits the hook) runs a native `lifecycle.preStop.sleep` so the Gateway drops the endpoint before SIGTERM arrives. `<component>.terminationGracePeriodSeconds` must exceed the sleep plus the app's drain time. The `sleep` action needs Kubernetes >= 1.30 (it is also why no `exec: sleep` is used: the sandbox-proxy image is distroless). `maxSurge: 1` briefly runs two pods per component. Checked by `mise run rollout`.

## Migration ordering
Jobs are plain resources, not Helm hooks (hooks would deadlock `helm install --wait`: pods would need schemas that only post-install hooks create). Init containers (`kubectl wait`, image `waitImage`) enforce the order: workflows waits for the pgop Cluster, Database and all Roles (`migrate --require-roles` needs the runtime roles), connectors waits for the workflows Job, ui for the connectors Job, and every runtime Deployment (ui, workflows, connectors) for the ui Job. Job names embed a hash of the Job's inputs (image, pull policy, DB names, wait image, security context, upstream Job name, a template revision), so any change creates a new Job and unchanged inputs keep the completed one.

## Database credentials
pgop creates `<cluster>-<role>-credentials` per Role (keys `username password host port sslmode`, `uri` pointing at `postgres`) and a per-database Secret only for the Database owner. The chart passes `PGHOST PGPORT PGUSER PGPASSWORD PGSSLMODE` from the Role Secret plus `PGDATABASE` from values, and sets `DATABASE_URL=postgres:///enzarb` (no credentials, so passwords with URL-special characters are safe). sqlx (workflows, connectors) falls back to the `PG*` variables. postgres.js (ui) honors all of them except `PGSSLMODE`, so the ui gets `DATABASE_URL=postgres:///enzarb?sslmode=$(PGSSLMODE)`. ui probes are HTTP (`/healthz`, `/readyz`); set `ui.probes.http=false` for TCP.

Chat is always on; it needs sandbox >= 0.8.0 and sandbox-proxy >= 0.4.0 (both pinned). `ENZARB_MAX_CHATS` (sandbox, default 1; 0 disables chat there so the Chat tab shows "not available") is the kill switch. The former `ui.chat.enabled` value is a deprecated no-op (the schema tolerates it for one release so a stale values file cannot break `helm upgrade`); remove it from your values.

The ui Deployment gets `ENZARB_COMPONENT_VERSIONS`, a single-line JSON object `{"chart": "<chart version>", "components": {"<repo name>": "<tag>"}}` for the About page, rendered from the same values the chart-sync writes. The component keys are the historical component (former repo) names; the UI maps each to its monorepo directory and `<dir>/v*` release tags: `enzarb-ui`, `enzarb-workflows`, `enzarb-connectors`, `enzarb-realtime` and `enzarb-updater` (only when enabled), `enzarb-sandbox-proxy`, `enzarb-sandbox-operator` and `enzarb-sandbox-container` (tag of `operator.sandboxImage`). `ui.extraEnv` can override it. Checked by `mise run componentversions`.

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
Every component image tag in `enzarb/values.yaml` (and `operator.sandboxImage`) is a pinned release version, never `main`. They are written by `scripts/sync-components.sh`, which reconciles each entry of `scripts/components.yaml` to the component's highest `<dir>/vX.Y.Z` tag in this monorepo, but only when that tag's `ci-<dir>` run succeeded (its docker job pushes `ghcr.io/enzarb/<image>:<version>`) and the version is newer than the current pin. The root `sync-components` workflow runs it when a component's tag build succeeds (and every 6 hours as a safety net), refreshes the goldens, runs the checks and commits `feat(deps)`/`fix(deps)` to `main`; the release workflow then tags `helm/vX.Y.Z` and `publish-helm.yml` pushes `oci://ghcr.io/enzarb/charts/enzarb`. `Chart.yaml` keeps `0.0.0` placeholders. See `docs/ci.md`.

`enzarb/values-oldie.yaml` is the single-node production install (`enzarb.dev`, StorageClass `enzarb-lvm`, `IfNotPresent`); render it with `scripts/render.sh oldie`.
