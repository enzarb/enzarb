# enzarb-sandbox-operator

Kubernetes operator for enzarb `Project`s. The workspace holds two crates:

- `crates/enzarb-crds`: the `Project` CRD types (`enzarb.dev/v1alpha1`, cluster-scoped), generated upstream types for Capsule `Tenant` and agent-sandbox `Sandbox`, and `crdgen` (`cargo run -p enzarb-crds --bin crdgen` writes `deploy/crds/project.yaml`).
- `crates/operator`: the controller binary `enzarb-sandbox-operator`.

Interfaces are defined in the enzarb repo, `docs/contracts.md` sections 2 and 3.

## Project lifecycle

For each `Project` the operator:

1. Adds the finalizer `enzarb.dev/cleanup`.
2. Server-side applies the Capsule `Tenant` `enzarb-<owner>` (owner = Capsule `User` `<owner>`). One Tenant per owner, shared by all of that owner's Projects.
3. Server-side applies the `Namespace` `enzarb-p-<name>`, labelled `capsule.clastix.io/tenant=enzarb-<owner>`, owned by the Project.
4. Creates Secret `sandbox-auth` (key `token`, 32 random bytes hex-encoded) once and never rewrites it, so the token does not rotate.
5. Server-side applies NetworkPolicy `sandbox-isolation` (owned by the Project) before any pod exists. It selects every pod in the namespace. Ingress: TCP 7070 only from pods whose `app.kubernetes.io/name` is in `--netpol-ingress-pod-names` (default `enzarb-sandbox-proxy`, `enzarb-workflows`) in namespaces matching the ingress namespace label (default `kubernetes.io/metadata.name=<release namespace>`); both selectors must match. Egress: DNS (UDP/TCP 53) and TCP 443, to any destination. A narrower egress allow-list is a later option. A failure here is reported as `NamespaceReady=False` (`ReconcileError`) and the sandbox is not created. This needs a CNI that enforces NetworkPolicy.
6. Applies PVC `workspace` (`spec.sandbox.storage`, else `--default-storage`) and the agent-sandbox `Sandbox` `sandbox`. The Sandbox pod runs as uid/gid 1000 (user `enzarb` in the sandbox image) with `fsGroup` 1000. The token is mounted read-only at `/var/run/enzarb/auth/token`, mode 0400. The pod has no ServiceAccount token.
7. Writes `status` (`namespace`, `sandbox.endpoint`, `sandbox.authSecretRef`) and the conditions `TenantReady`, `NamespaceReady`, `SandboxReady`, `Ready`. A failed step is `status: False` with a reason ending in `Failed`, `Failure` or `Error`.

On deletion the operator deletes the project namespace (only if it carries this Project's labels), which removes the Secret, PVC and Sandbox. The Tenant is never deleted: doing so would cascade to the owner's other namespaces.

An invalid spec (bad derived DNS name, empty owner, no image available) is reported in the `TenantReady` condition with reason `InvalidSpec...` and is not retried until the spec changes. API errors retry with exponential backoff (5s up to 5m).

## Flags and environment

Each flag has an environment variable; the flag wins.

| Flag | Env | Default | Meaning |
|---|---|---|---|
| `--sandbox-image` | `ENZARB_SANDBOX_IMAGE` | empty | Image when `spec.sandbox.image` is empty. If both are empty the Project is invalid. |
| `--default-storage` | `ENZARB_DEFAULT_STORAGE` | `20Gi` | PVC size when `spec.sandbox.storage` is empty. |
| `--release-namespace` | `ENZARB_RELEASE_NAMESPACE` | pod namespace | Namespace enzarb is released into; used for the default ingress namespace label. |
| `--netpol-ingress-namespace-label` | `ENZARB_NETPOL_INGRESS_NAMESPACE_LABEL` | `kubernetes.io/metadata.name=<release namespace>` | `key=value` label of the namespace(s) allowed to reach sandbox port 7070. |
| `--netpol-ingress-pod-names` | `ENZARB_NETPOL_INGRESS_POD_NAMES` | `enzarb-sandbox-proxy,enzarb-workflows` | Comma-separated `app.kubernetes.io/name` values of allowed client pods. |
| `--health-addr` | `ENZARB_HEALTH_ADDR` | `0.0.0.0:8080` | Serves `/healthz` and `/readyz`. |
| `--leader-election` | `ENZARB_LEADER_ELECTION` | `true` | Take a Lease before running the controller. Pass `false` for local runs. |
| `--leader-election-name` | `ENZARB_LEADER_ELECTION_NAME` | `enzarb-sandbox-operator` | Lease name. |
| `--leader-election-namespace` | `ENZARB_LEADER_ELECTION_NAMESPACE` | pod namespace | Lease namespace (read from the ServiceAccount mount by default). |
| `--identity` | `POD_NAME` | hostname | Holder identity prefix; a random suffix is added. Set `POD_NAME` from the downward API. |
| `--lease-duration-secs` | `ENZARB_LEASE_DURATION_SECS` | `15` | How long a Lease is valid without renewal. |
| `--lease-renew-secs` | `ENZARB_LEASE_RENEW_SECS` | `5` | Renew (and standby retry) interval. Must be below the duration. |

`RUST_LOG` sets the log filter (default `info`); logs are JSON.

## Leader election

The operator can run 2 replicas. All replicas serve health (`/readyz` is ready on standbys) and compete for a `coordination.k8s.io/v1` Lease. Only the holder runs the controller. The holder renews every 5s; if a renewal does not succeed for `duration - renew` (10s) or another holder appears, the process exits so it restarts as a standby. A standby takes over when the Lease is released (clean shutdown does this) or has not been renewed for the lease duration. Takeover races are resolved by the Lease `resourceVersion`.

## RBAC

`deploy/rbac.yaml` has a ClusterRole `enzarb-sandbox-operator` and a Role `enzarb-sandbox-operator-leader` (leases; bind in the operator's namespace). The ServiceAccount and bindings belong to enzarb-helm; the file header shows the binding subjects. `crates/operator/src/rbac.rs` lists every API call the code makes, and a unit test asserts the YAML grants exactly that, so when you add a call, update both.

| Resource | Verbs | Why |
|---|---|---|
| `enzarb.dev` projects | get, list, watch, patch | controller, finalizer |
| `enzarb.dev` projects/status | patch | status and conditions |
| `capsule.clastix.io` tenants | create, patch | server-side apply |
| namespaces | get, create, patch, delete | apply, ownership check, cleanup |
| secrets | get, create | one-time token |
| persistentvolumeclaims | create, patch | server-side apply |
| `agents.x-k8s.io` sandboxes | list, watch, create, patch | owned-resource watch, apply |
| `coordination.k8s.io` leases (Role) | get, create, update | leader election |

The operator emits no Kubernetes Events today, so none are granted.

## Capsule requirement

The operator creates Namespaces carrying the tenant label itself rather than going through a tenant owner. Capsule only admits that for identities in its configured `userGroups`, so the operator's ServiceAccount must be covered: add `system:serviceaccounts:<operator namespace>` (or its user, `system:serviceaccount:<ns>:<name>`) to the CapsuleConfiguration `userGroups`. Without it, `NamespaceReady` stays failed with a Capsule webhook error.

## Image

`docker build .` produces a static musl binary on Alpine, running as uid/gid 65532, entrypoint `/usr/local/bin/enzarb-sandbox-operator`, health on 8080.

## Development

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
docker build .
```

Run locally against your current kube context with `cargo run -p enzarb-sandbox-operator -- --leader-election=false --sandbox-image <image>`.
