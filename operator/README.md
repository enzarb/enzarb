# enzarb-sandbox-operator

Kubernetes operator for enzarb `Project`s. The workspace holds two crates:

- `crates/enzarb-crds`: the `Project` CRD types (`enzarb.dev/v1alpha1`, cluster-scoped), generated upstream types for the agent-sandbox `Sandbox`, per-Project name helpers (`sandbox_name()`, `auth_secret_name()`, `workspace_pvc_name()`, `docker_pvc_name()`, `network_policy_name()`), and `crdgen` (`cargo run -p enzarb-crds --bin crdgen` writes `deploy/crds/project.yaml`).
- `crates/operator`: the controller binary `enzarb-sandbox-operator`.

Interfaces are defined in the enzarb repo, `docs/contracts.md` sections 2 and 3.

## Project lifecycle (ADR-0013)

All Projects land in one shared namespace (`--sandbox-namespace`, default `enzarb-sandboxes`, created by the Helm chart). The operator does not create namespaces or Capsule Tenants. For each `Project` `<name>` (a DNS label of at most 50 characters, so `sb-<name>-workspace` fits 63) it applies, in order, each labelled `app.kubernetes.io/managed-by=enzarb-operator` and `enzarb.dev/project=<name>` and owned by the Project (controller ownerReference with `blockOwnerDeletion` on every object). **Ownership check:** before reusing an existing Secret, PVC, NetworkPolicy or Sandbox the operator requires its controller ownerReference uid to equal the Project's uid and no `deletionTimestamp`; otherwise it reports `ResourcesReady=False` (or `SandboxReady=False`) with reason `Terminating` (being deleted, or controlled by another Project uid) and requeues after 5s. An object with no controller ownerReference at all is reported with reason `NotOwnedError` (the message names it; an administrator must delete it) and requeued every 60s. So a Project re-created under the same name never reuses its predecessor's Secret or volumes:

1. NetworkPolicy `sb-<name>`, before any pod exists. `podSelector` is `enzarb.dev/project=<name>` (only this Project's pod). Ingress: TCP 7070 only from pods whose `app.kubernetes.io/name` is in `--netpol-ingress-pod-names` (default `enzarb-sandbox-proxy`, `enzarb-workflows`) in namespaces matching the ingress namespace label (default `kubernetes.io/metadata.name=<release namespace>`); both selectors must match. Egress: DNS (UDP/TCP 53) to `k8s-app=kube-dns` pods in `kube-system`, plus any `--dns-cidrs`; TCP 443 to `0.0.0.0/0` and `::/0` except `--egress-exclude-cidrs` (required, and, like `--dns-cidrs`, rejected at startup if a CIDR is `/0` or has host bits set: the cluster's pod, service and node networks, split into the matching IPv4/IPv6 `except` lists; the operator refuses to start without it); TCP `--executor-callback-port` (default 8081) to the `enzarb-workflows` pods in the clients' namespace. The Kubernetes API server is therefore not reachable (policies are evaluated after DNAT; its Service and endpoint addresses are in the excluded networks). A namespace-wide default-deny policy is owned by enzarb-helm. This needs a CNI that enforces NetworkPolicy.
2. Secret `sb-<name>-auth` (key `token`, 32 random bytes hex-encoded), created once and never rewritten, so the token does not rotate.
3. PVCs `sb-<name>-workspace` (`spec.sandbox.storage`, else `--default-storage`) and `sb-<name>-docker` (`--docker-storage`).
4. The agent-sandbox `Sandbox` `sb-<name>` (and its headless Service of the same name). The pod runs as ServiceAccount `--sandbox-service-account` (default `enzarb-sandbox`, no RBAC) with `automountServiceAccountToken: false` and `enableServiceLinks: false`, so **sandboxes have no Kubernetes API access**; any future access is limited to other namespaces. It runs as uid/gid 1000 (user `enzarb`) with `fsGroup` 1000 (`fsGroupChangePolicy: OnRootMismatch`, so the large docker PVC is not re-chowned on every start). The token is mounted read-only at `/var/run/enzarb/auth/token`, mode 0400. If `spec.maxRuns` (integer 1-16, enforced by the CRD schema) is set, the sandbox container gets `ENZARB_MAX_RUNS=<n>` (the concurrent agent run limit); when unset nothing is injected and the image default (2) applies. Changing `spec.maxRuns` recreates the pod like any pod-shape change (see "Applying pod-shape changes"). `spec.sandbox.cpu` (integer 1-64) and `spec.sandbox.memory` (`<int>Mi` or `<int>Gi`, 512Mi-256Gi) set the sandbox container's resource limits; the requests are 25% of the limit (at least `100m` CPU / `256Mi` memory). An explicit `spec.sandbox.resources` wins for every key it sets (a derived request above an explicit limit is clamped to it). Unset, nothing is injected (the container gets only `spec.sandbox.resources`, as before). A value outside the range is `InvalidSpec` on `ResourcesReady`.
5. Status: `status.namespace` is the shared namespace; `status.sandbox` has `name: sb-<name>`, `endpoint: ws://sb-<name>.<namespace>.svc:7070/v1/mux` and `authSecretRef: {name: sb-<name>-auth, key: token}`.

Conditions, in order: `ResourcesReady` (NetworkPolicy, Secret and both PVCs applied), `SandboxReady` (the Sandbox's own `Ready`), `Ready` (all of them). A failed step is `status: False` with a reason ending in `Error`. The CRD itself rejects, on create only (`optionalOldSelf`, so existing Projects stay updatable), names over 50 characters or that are not DNS labels (CEL rules on `metadata.name`). The operator also reports an invalid spec (name not a DNS label of at most 50 characters, empty owner, no image available) is `ResourcesReady=False` with reason `InvalidSpec` and is not retried until the spec changes. API errors retry with exponential backoff (5s up to 5m) and surface as `status: Unknown` with reason `Retrying` (not a failure reason, so the UI keeps polling). Sandboxes (namespaced) and Projects are watched; the Secret, PVCs and NetworkPolicy are re-checked on each reconcile (Sandbox or Project change, the 5 minute resync, or the requeues above). A change to a Project also re-reconciles every other Project that shares a repository with it (see below).

**One Project per repository.** If several Projects list the same repository (compared by connector and normalized path: case-insensitive, ignoring `.git` and a trailing `/`; `defaultBranch` is ignored), only the oldest by `(creationTimestamp, name)` reconciles normally. Every other one gets `ResourcesReady=False` with reason `InvalidSpec`, a message naming only the repository, and no resources are created for it. Resources a Project already has when it becomes a loser are left untouched. A loser is re-evaluated when the older Project is changed or deleted (a Project being deleted no longer holds its repositories). The check reads the operator's Projects cache, which can lag by a moment, so a conflict may be reported or cleared slightly late; the watch and the resync correct it. The ordering is `enzarb_crds::cmp_precedence`, for other components to reuse.

**Deletion** needs no finalizer, and the operator adds none: every object is owned by the Project, so garbage collection removes them (the volumes and their data included) when the Project is deleted.

**Migration from the Capsule layout** is not done by the operator. The workspace repo's one-off task `mise run oldie:migrate-0013` removes the old `enzarb.dev/cleanup` finalizer from existing Projects and deletes the Capsule Tenants the old operator created (ADR-0013).

## Flags and environment

Each flag has an environment variable; the flag wins.

| Flag | Env | Default | Meaning |
|---|---|---|---|
| `--sandbox-image` | `ENZARB_SANDBOX_IMAGE` | empty | Image when `spec.sandbox.image` is empty. If both are empty the Project is invalid. |
| `--sandbox-roll-drain-timeout` | `ENZARB_SANDBOX_ROLL_DRAIN_TIMEOUT` | `2h` | How long a pending pod-shape change waits for the drain ack or idleness before the pod is deleted anyway. Accepts `30s`, `10m`, `2h` or plain seconds. |
| `--default-storage` | `ENZARB_DEFAULT_STORAGE` | `20Gi` | PVC size when `spec.sandbox.storage` is empty. |
| `--release-namespace` | `ENZARB_RELEASE_NAMESPACE` | pod namespace | Namespace enzarb is released into (where the sandbox clients and the executor run); used for the default ingress namespace label. |
| `--sandbox-namespace` | `ENZARB_SANDBOX_NAMESPACE` | `enzarb-sandboxes` | Shared namespace for all Projects' objects. Must exist (Helm chart). |
| `--sandbox-service-account` | `ENZARB_SANDBOX_SERVICE_ACCOUNT` | `enzarb-sandbox` | ServiceAccount of sandbox pods (no token, no RBAC). |
| `--executor-callback-port` | `ENZARB_EXECUTOR_CALLBACK_PORT` | `8081` | TCP port of the workflows executor callback listener that sandboxes may reach. |
| `--egress-exclude-cidrs` | `ENZARB_SANDBOX_EGRESS_EXCLUDE_CIDRS` | none, required | Comma-separated CIDRs excluded from the TCP 443 egress rule: cluster pod, service and node networks. The operator refuses to start if empty. |
| `--dns-cidrs` | `ENZARB_SANDBOX_DNS_CIDRS` | empty | Extra DNS server CIDRs allowed on port 53 (e.g. `169.254.20.10/32` for NodeLocal DNSCache). |
| `--netpol-ingress-namespace-label` | `ENZARB_NETPOL_INGRESS_NAMESPACE_LABEL` | `kubernetes.io/metadata.name=<release namespace>` | `key=value` label of the namespace(s) allowed to reach sandbox port 7070. |
| `--netpol-ingress-pod-names` | `ENZARB_NETPOL_INGRESS_POD_NAMES` | `enzarb-sandbox-proxy,enzarb-workflows` | Comma-separated `app.kubernetes.io/name` values of allowed client pods. |
| `--sandbox-runtime-class` | `ENZARB_SANDBOX_RUNTIME_CLASS` | none, required | RuntimeClass of every sandbox pod (recommended `gvisor`). Must be in `--docker-runtime-classes`; the operator refuses to start otherwise. |
| `--docker-runtime-classes` | `ENZARB_DOCKER_RUNTIME_CLASSES` | `gvisor` | RuntimeClass names allowed to host dockerd. Each needs a profile. |
| `--docker-storage` | `ENZARB_DOCKER_STORAGE` | `50Gi` | Size of each Project's PVC `sb-<name>-docker`. |
| `--docker-profiles-file` | `ENZARB_DOCKER_PROFILES_FILE` | none | YAML map `{<runtimeClass>: {securityContext, dockerdArgs}}` added over the built-in profiles. `securityContext` is the `sandbox` container's. |
| `--docker-prune` | `ENZARB_DOCKER_PRUNE` | unset (sandbox default: `on`) | `on`/`off`: sandbox-init prunes Docker images, build cache and volumes automatically. Passed to the sandbox as `ENZARB_DOCKER_PRUNE`. |
| `--docker-prune-age` | `ENZARB_DOCKER_PRUNE_AGE` | unset (sandbox default: `24h`) | Images and build cache newer than this survive a prune. |
| `--docker-prune-min-free` | `ENZARB_DOCKER_PRUNE_MIN_FREE` | unset (sandbox default: `15`) | Percent of `/var/lib/docker` that must stay free; below it the sandbox prunes without the age filter and `/healthz` reports `low_disk`. |

Removed with ADR-0016 (dockerd no longer has its own container): `--docker-image`, `--docker-cpu-request`/`--docker-cpu-limit`, `--docker-memory-request`/`--docker-memory-limit`. dockerd and inner containers share the `sandbox` container's resources (`spec.sandbox.resources`).
| `--health-addr` | `ENZARB_HEALTH_ADDR` | `0.0.0.0:8080` | Serves `/healthz` and `/readyz`. |
| `--leader-election` | `ENZARB_LEADER_ELECTION` | `true` | Take a Lease before running the controller. Pass `false` for local runs. |
| `--leader-election-name` | `ENZARB_LEADER_ELECTION_NAME` | `enzarb-sandbox-operator` | Lease name. |
| `--leader-election-namespace` | `ENZARB_LEADER_ELECTION_NAMESPACE` | pod namespace | Lease namespace (read from the ServiceAccount mount by default). |
| `--identity` | `POD_NAME` | hostname | Holder identity prefix; a random suffix is added. Set `POD_NAME` from the downward API. |
| `--lease-duration-secs` | `ENZARB_LEASE_DURATION_SECS` | `15` | How long a Lease is valid without renewal. |
| `--lease-renew-secs` | `ENZARB_LEASE_RENEW_SECS` | `5` | Renew (and standby retry) interval. Must be below the duration. |

`RUST_LOG` sets the log filter (default `info`); logs are JSON.

## Applying pod-shape changes (drain first)

Updating the Sandbox template does not recreate a running pod. The operator compares the **desired pod shape** with the running pod: the image of every container, `ENZARB_*` env vars of the `sandbox` container (including `ENZARB_MAX_RUNS` and `ENZARB_DOCKERD_ARGS`), container resources (`requests` and `limits`, compared as quantities so `1Gi` equals `1024Mi`), the container's `securityContext` identity and capabilities (`runAsUser`, `privileged`, `allowPrivilegeEscalation`, `capabilities.add`), `runtimeClassName`, and the operator-defined volume mounts. Only keys the desired template sets are compared (defaults and fields injected into the live pod never count, so an operator upgrade does not roll existing pods), except that an `ENZARB_*` var the pod carries and the operator no longer sets does count. A new field joins by adding one line to `shape_of` in `roll.rs`. Storage growth is not a pod-shape change (the PVC expands in place).

On a difference the pod is **left alone** and the change is queued: `status.pendingChange = {generation, since, reason}` plus the informational condition `SandboxChangePending` (not part of the UI checklist, no effect on `Ready`; this replaces `SandboxImageRoll`). The pod is deleted (with a UID precondition; never the Sandbox or the PVCs) only when one of these holds, and the pod is at least two minutes old:

1. the Project annotation `enzarb.dev/drained-generation` equals `status.pendingChange.generation` (the drain ack; set by workflows after it has drained the sandbox). Any other value, including an old, future or non-numeric one, is ignored. A new pending change always gets a generation above any ack already on the Project, so a stale ack never releases it;
2. the pod carries `enzarb.dev/active-sessions` = `0` (idle);
3. `--sandbox-roll-drain-timeout` (default `2h`) has passed since `pendingChange.since`.

A further spec change while one is pending bumps `generation` (so an ack for the older one no longer applies) and keeps `since`, so a stream of edits cannot postpone the apply forever. `pendingChange` stays until the recreated pod matches the template, then it and the condition are cleared. A pod that was recreated and still differs is not deleted again for the same difference.

## Docker in the sandbox (ADR-0012, ADR-0016)

Docker is always on, and Projects cannot choose the runtime class: both are cluster-admin settings (operator flags). Every Sandbox pod gets `runtimeClassName: <--sandbox-runtime-class>` and one container, `sandbox`, in which sandbox-init (pid 1) runs and supervises `dockerd` (there is no sidecar: under gVisor a socket cannot cross containers through an `emptyDir`, and bind mounts need the agent's filesystem). The operator adds to the `sandbox` container:

- the runtime profile's `securityContext`, which overrides the pod-level `runAsUser`/`runAsNonRoot` (sandbox-init starts as root and drops every child it spawns to uid 1000; the pod-level `runAsUser`, `runAsGroup`, `fsGroup` and `fsGroupChangePolicy` stay so the workspace and token keep their ownership);
- `DOCKER_HOST=unix:///var/run/docker.sock` (dockerd listens only there, group 1000, mode 0660; no TCP listener) and `ENZARB_DOCKERD_ARGS` (the profile's `dockerdArgs`, space separated, so an argument cannot contain whitespace);
- PVC `sb-<name>-docker` (`--docker-storage`, separate from the workspace PVC) as volume `docker-data` mounted at `/var/lib/docker`.

An existing sandbox from the sidecar layout differs from this shape (new mount, env and security context), so it drains and rolls once through the drain-first path above.

**Isolation gate (startup).** The operator refuses to start, with a clear error, if `--sandbox-runtime-class` is empty, is not in `--docker-runtime-classes`, or an allowed class has no profile. dockerd is therefore never scheduled on the node kernel. There is no per-Project refusal condition.

**Runtime table** (built in; extend or override with `--docker-profiles-file`). Only `gvisor` is supported and allowed by default; `kata` is an experimental entry (it must also be added to `--docker-runtime-classes`) that is untested here:

| Runtime class | `sandbox` container securityContext | Extra dockerd args |
|---|---|---|
| `gvisor` | not privileged, root, `capabilities: drop ALL, add AUDIT_WRITE CHOWN DAC_OVERRIDE FOWNER FSETID KILL MKNOD NET_BIND_SERVICE NET_ADMIN NET_RAW SETFCAP SETGID SETPCAP SETUID SYS_ADMIN SYS_CHROOT SYS_PTRACE` | `--iptables=false --ip6tables=false --storage-driver=overlay2 --feature=containerd-snapshotter=false` |
| `kata` | `privileged: true`, root (confined to the guest VM) | `--storage-driver=overlay2` |

Sources: [Docker in gVisor](https://gvisor.dev/docs/tutorials/docker-in-gvisor/) (capability set, `--iptables=false --ip6tables=false`, v29 `containerd-snapshotter=false`, runsc `--net-raw` and, for Docker 28+, `--allow-packet-socket-write`) and Kata's [privileged containers guide](https://github.com/kata-containers/kata-containers/blob/main/docs/how-to/privileged.md) (`privileged_without_host_devices = true` so privilege stays inside the guest; kata-deploy sets it).

**Cluster requirements.**

- gVisor: the `runsc` config behind RuntimeClass `gvisor` must set `net-raw = "true"` and `allow-packet-socket-write = "true"` under `[runsc_config]` (containerd runtime options). Without them dockerd's networking fails.
- Kata (experimental): containerd must set `privileged_without_host_devices = true` for the Kata runtime (kata-deploy does); nodes need KVM.
- Nodes with the runtime should be labelled/tainted and the RuntimeClass given `scheduling`.

**Limitations.**

- gVisor: iptables is disabled, so `docker run -p/--expose` publishes nothing; use `--network=host` inside the sandbox, or reach containers by IP. User-defined networks and compose port publishing are limited accordingly. Some syscalls and filesystems are unsupported by gVisor.
- gVisor storage uses `overlay2` (verified on the docker volume, issue #119). sandbox-init deletes the leftover `vfs` directory of the old default at startup. To go back, set `runtimeProfiles.<class>.dockerdArgs` with `--storage-driver=vfs`.
- Kata (experimental): `overlay2` needs the `docker` PVC to be mounted in the guest as a real block filesystem (ext4/xfs), not virtio-fs; with a virtio-fs-backed volume, override the profile to `--storage-driver=vfs`.
- The `sandbox` container runs as root with `runAsNonRoot: false` (dockerd requires it); agents, jobs and chats run as uid 1000 with no-new-privs (sandbox-init drops them). dockerd, containerd and inner containers count against the container's CPU and memory limits, so a heavy build can OOM the whole container. Inner containers and builds default to `--network=host` through the image's `docker` wrapper; use `--user 1000:1000` for bind-mounted outputs you want to edit (otherwise they are root-owned). Plain-HTTP registries are blocked by the Project's NetworkPolicy (443, DNS and the executor callback only).

## Workspace sizing

The workspace PVC `sb-<name>-workspace` holds the repo checkout, per-run worktrees and toolchains. Budget several GB of build output (e.g. cargo `target/`) per concurrent run plus about 3G of toolchains; for Rust monorepos use at least `40Gi`.

- **Setting the size:** `spec.sandbox.storage`. If empty, the Project gets the default (`20Gi`).
- **Growing:** raise `spec.sandbox.storage`; the operator re-applies the PVC on every reconcile. The StorageClass must set `allowVolumeExpansion: true`. PVCs cannot shrink.
- **Status:** while the volume is being expanded, `ResourcesReady` is `False` with reason `WorkspaceResizing`. If the API server refuses the change (a shrink, or a StorageClass without expansion), `ResourcesReady` is `False` with reason `WorkspaceResizeFailed` and the server's message. Restore the old size or enable expansion to fix it.
- **Changing the default:** if the default is ever changed, the PVC of every Project that does not set `storage` is grown (or reported as `WorkspaceResizeFailed` if the StorageClass cannot expand). Set `storage` explicitly to pin a size.
- The operator cannot see disk usage, so there is no usage-high condition.

## Leader election

The operator can run 2 replicas. All replicas serve health (`/readyz` is ready on standbys) and compete for a `coordination.k8s.io/v1` Lease. Only the holder runs the controller. The holder renews every 5s; if a renewal does not succeed for `duration - renew` (10s) or another holder appears, the process exits so it restarts as a standby. A standby takes over when the Lease is released (clean shutdown does this) or has not been renewed for the lease duration. Takeover races are resolved by the Lease `resourceVersion`.

## RBAC

`deploy/rbac.yaml` has a ClusterRole `enzarb-sandbox-operator` (Projects, migration), a Role `enzarb-sandbox-operator` for the sandbox namespace, and a Role `enzarb-sandbox-operator-leader` (leases; bind in the operator's namespace). The ServiceAccount and bindings belong to enzarb-helm; the file header shows the binding subjects. `crates/operator/src/rbac.rs` lists every API call the code makes, and a unit test asserts the YAML grants exactly that, so when you add a call, update both. The operator has no `namespaces` permission and no cluster-wide access to Secrets, PVCs, NetworkPolicies or Sandboxes.

| Scope | Resource | Verbs | Why |
|---|---|---|---|
| ClusterRole | `enzarb.dev` projects | list, watch | controller |
| ClusterRole | `enzarb.dev` projects/status | patch | status and conditions |
| ClusterRole | `enzarb.dev` projects/finalizers | update | ownerReferences with `blockOwnerDeletion` |
| Role, sandbox namespace | secrets | get, create | one-time token; `get` for the ownership check |
| Role, sandbox namespace | persistentvolumeclaims | get, create, patch | server-side apply (workspace, docker) |
| Role, sandbox namespace | `networking.k8s.io` networkpolicies | get, create, patch | server-side apply |
| Role, sandbox namespace | `agents.x-k8s.io` sandboxes | get, list, watch, create, patch | owned-resource watch (namespaced), apply |
| Role, operator namespace | `coordination.k8s.io` leases | get, create, update | leader election |

The operator emits no Kubernetes Events today, so none are granted.

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
