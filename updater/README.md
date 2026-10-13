# enzarb-updater

Keeps the cluster on the latest enzarb chart. It patches `spec.version` of the RKE2 `HelmChart` and the helm-controller upgrades. No database, no state, one binary.

Two triggers, both applying through the same never-downgrade, serialized logic:

- **Polling (default, needs nothing inbound).** Every `ENZARB_UPDATER_POLL_SECS` it lists the chart's tags in the OCI registry (`/v2/<repo>/tags/list`, all pages, same bearer-token flow), takes the highest plain `X.Y.Z` tag (prerelease, build metadata, `v` prefixes and non-semver tags are skipped) and applies it.
- **Webhook (optional).** `POST /hook` (GitHub `release` webhook), only routed when `ENZARB_UPDATER_WEBHOOK_SECRET` is set; otherwise `/hook` is 404.

`GET /healthz` (liveness, always ok) and `GET /readyz` (503 while the last poll failed or has not run yet; always ok when polling is off) on `:8080`.

The `HelmChart` is never created by the updater (the RBAC grants no `create`): if it does not exist it logs `HelmChart does not exist` and does nothing. The cluster bootstrap creates it once.

## Webhook request flow

1. Verify `X-Hub-Signature-256` (HMAC-SHA256, constant-time) with `ENZARB_UPDATER_WEBHOOK_SECRET`. 401 otherwise.
2. `X-GitHub-Event: ping` -> 200. Any other event except `release` -> 202 ignored.
3. Act only on `action == "published"`, `repository.full_name == "enzarb/enzarb"`, a non-prerelease, non-draft tag `helm/vX.Y.Z` (the `helm/` prefix is stripped for the version; other components' tags are ignored). Everything else -> 202 ignored.
4. HEAD the chart manifest `X.Y.Z` in the OCI registry (anonymous bearer-token flow, or authenticated with `ENZARB_UPDATER_REGISTRY_TOKEN`). Missing or registry error -> 503 so GitHub redelivery retries.
5. Read the `HelmChart`. If `X.Y.Z` is greater than `spec.version`, server-side apply only `spec.version` (field manager `enzarb-updater`, forced). Equal or lower -> 200 no-op. It never downgrades.

Every decision is logged as one JSON line (`decision` = `ping|ignore|reject|retry|noop|patched|error`).

## Configuration

| Var | Default | |
|---|---|---|
| `ENZARB_UPDATER_WEBHOOK_SECRET` | unset | GitHub webhook secret; unset disables `/hook` (404) |
| `ENZARB_UPDATER_POLL_SECS` | `300` | registry poll interval; `0` disables polling, otherwise at least 30. At least one of secret and polling is required |
| `ENZARB_UPDATER_CHART` | `oci://ghcr.io/enzarb/charts/enzarb` | chart ref the monorepo's chart publish workflow pushes |
| `ENZARB_UPDATER_REGISTRY_TOKEN` | unset | token with `read:packages` (the GHCR chart is private); sent only to the registry host over https |
| `ENZARB_UPDATER_HELMCHART` | `enzarb` | `HelmChart` name |
| `ENZARB_UPDATER_NAMESPACE` | `kube-system` | `HelmChart` namespace |
| `ENZARB_UPDATER_LISTEN` | `0.0.0.0:8080` | |
| `RUST_LOG` | `info` | JSON logs |

## RBAC

A Role (and RoleBinding to the updater's ServiceAccount) in the HelmChart namespace:

```yaml
rules:
  - apiGroups: [helm.cattle.io]
    resources: [helmcharts]
    resourceNames: [enzarb]
    verbs: [get, patch]
```

## Ordering requirement

The monorepo's chart publish workflow (triggered by the `helm/v*` tag) must push the OCI chart **before** the GitHub release is published, otherwise the first delivery gets 503. GitHub does not retry failed deliveries automatically, so redeliver manually or publish in order. The updater only checks; it does not wait.

## Operational notes

- Run a single replica (updates are serialized in-process only).
- The updater takes ownership of `spec.version` (forced server-side apply). If the chart template also renders the `HelmChart`, leave `spec.version` out of it or helm upgrades will fight the updater.
- `spec.version` must be plain semver (a leading `v` is tolerated); a range or `*` makes every delivery fail with 500 and is never overwritten.
- The `HelmChart` must already exist; a missing one is logged and left alone (webhook answers 200, a poll counts as successful).

## Build

`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`, `docker build .`. Tools via `mise install`.
