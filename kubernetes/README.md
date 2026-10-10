# enzarb-kubernetes

Cluster prerequisites for enzarb, as kustomize bases plus a kind dev cluster. Everything
the `enzarb-helm` chart assumes already exists is installed from here. Nothing in this repo
deploys enzarb itself.

## Layout

```
kind/cluster.yaml          kind: 1 control-plane + 1 worker, host 80/443 -> NodePorts 30080/30443
base/cert-manager          cert-manager (remote, pinned release asset)
base/envoy-gateway         Envoy Gateway install (remote) + GatewayClass/Gateway `enzarb`
  install/ config/           the two halves, applied separately (CRDs first)
base/agent-sandbox         kubernetes-sigs/agent-sandbox controller + Sandbox CRD (remote)
base/capsule               Capsule (vendored from the Helm chart; crds/ applied first)
base/capsule-proxy         capsule-proxy (vendored; optional, needs base/capsule)
base/pgop                  pgop operator (vendored from the Helm chart; crds/ applied first)
base/openebs-lvm           OpenEBS LVM LocalPV (one logical volume per PVC from a node volume group), driver image pinned
base/cluster-issuers       Let's Encrypt ClusterIssuers (staging + prod), DNS-01 through Cloudflare
base/cloudflared           cloudflared connector for a Cloudflare Tunnel (outbound-only public access)
base/runtimeclasses        RuntimeClass `gvisor` (handler runsc, nodeSelector enzarb.dev/runtime-gvisor=true)
base/gvisor-installer      privileged DaemonSet that installs gVisor on real nodes (prod; opt-in label)
overlays/dev               everything for kind (Envoy Service pinned to NodePorts) + gvisor RuntimeClass
overlays/oldie             single-node production install on oldie (RKE2): LVM storage, Let's Encrypt via Cloudflare DNS-01,
                             Cloudflare Tunnel; see ../docs/oldie.md in the workspace repo
overlays/prod              skeleton: dev set + gvisor RuntimeClass + gvisor-installer, TODOs inline
hack/versions.env          pinned versions (single list)
hack/refresh-vendored.sh   regenerates base/{capsule,capsule-proxy,pgop} from the Helm charts
hack/kind-install-gvisor.sh  installs gVisor into the kind nodes (dev)
hack/runsc.toml             runsc flags, shared by the kind script and the installer image
installer/                  Dockerfile + install.sh of ghcr.io/enzarb/enzarb-gvisor-installer
hack/pin-installer-digest.sh  pins the installer image in base/gvisor-installer by digest
hack/validate.sh           what CI runs (kubectl kustomize + kubeconform)
```

## Pinned versions

| Component | Version | Source | Bump |
|---|---|---|---|
| cert-manager | v1.21.2 | release `cert-manager.yaml` | edit URL in `base/cert-manager/kustomization.yaml` + `hack/versions.env` |
| Envoy Gateway | v1.9.2 | release `install.yaml` (includes Gateway API CRDs) | edit URL in `base/envoy-gateway/install/kustomization.yaml` + `versions.env` |
| agent-sandbox | v0.5.6 | release `sandbox.yaml` (core; extensions not installed) | edit URL in `base/agent-sandbox/kustomization.yaml` + `versions.env`; regenerate kopium types in `enzarb-sandbox-operator` |
| Capsule | chart/app 0.14.6 | Helm chart, vendored | edit `versions.env`, run `hack/refresh-vendored.sh` |
| capsule-proxy | chart/app 0.15.0 | Helm chart, vendored | same |
| gVisor | 20261005.0 | github.com/google/gvisor release tarball, sha512 pinned | edit `versions.env`; CI rebuilds the installer image from it (then run `hack/pin-installer-digest.sh` after merge) |
| pgop | chart/app 0.9.1 | `oci://ghcr.io/ruckc/charts/pgop`, vendored | same |
| RKE2 | v1.35.9+rke2r1 | rancher/rke2 release tarball, sha256-verified by the Ansible role | edit `RKE2_VERSION` in `versions.env`, run `mise run oldie:host` |
| OpenEBS LVM LocalPV | v1.10.1 | release `lvm-operator.yaml`; the floating `openebs/lvm-driver:ci` image is pinned to `1.10.1` in `base/openebs-lvm` | edit URL and `images:` in `base/openebs-lvm/kustomization.yaml` + `OPENEBS_LVM_VERSION` |
| cloudflared | 2026.10.0 | `docker.io/cloudflare/cloudflared` image in `base/cloudflared` | edit the image tag + `CLOUDFLARED_VERSION` |

agent-sandbox: v0.5.x exists (v0.5.6 is the last; v1.0.x is out) and ADR-0002 pins v0.5.x, so v0.5.6 is used.
Vendored bases are generated with Helm hooks disabled (the CRD-lifecycle, post-install and
pre-delete Jobs); CRDs are static in `crds/`. CI fails if `base/` drifts from the chart output.

## Dev cluster

```sh
kind create cluster --config kind/cluster.yaml
echo '127.0.0.1 enzarb.local' | sudo tee -a /etc/hosts   # how enzarb.local resolves
```

`https://enzarb.local` (and `http://`) then reaches the Envoy Gateway through the kind port
mappings. The certificate is self-signed (cert-manager `Issuer` `enzarb-selfsigned`), so browsers
and curl need `-k` or the CA from secret `enzarb-system/enzarb-tls` (`ca.crt`).

## Apply order

CRDs must be Established before objects that use them, and cert-manager's webhook must be up
before any `Certificate`/`Issuer`. Apply in stages (use `--server-side`: some CRDs are large):

```sh
k() { kubectl apply --server-side --force-conflicts "$@"; }

k -k base/cert-manager
kubectl -n cert-manager wait --for=condition=Available deploy --all --timeout=5m

k -k base/envoy-gateway/install
kubectl wait --for=condition=Established crd/gateways.gateway.networking.k8s.io \
  crd/envoyproxies.gateway.envoyproxy.io --timeout=2m
kubectl -n envoy-gateway-system wait --for=condition=Available deploy/envoy-gateway --timeout=5m

k -k base/agent-sandbox
k -k base/capsule/crds && kubectl wait --for=condition=Established crd/tenants.capsule.clastix.io --timeout=2m
k -k base/pgop/crds     && kubectl wait --for=condition=Established crd/clusters.pgop.ruck.io --timeout=2m

k -k overlays/dev          # everything else: Gateway config + dev NodePort patch, Capsule, capsule-proxy, pgop
kubectl -n capsule-system wait --for=condition=Available deploy --all --timeout=5m
```

Once the CRDs exist the last command is idempotent. On a fresh cluster a single `-k overlays/dev` fails on its
first pass only because of the missing CRDs and cert-manager webhook; the staged commands avoid that.

## Verify

```sh
kubectl -n cert-manager get deploy                                  # 3 Available
kubectl get gatewayclass enzarb                                     # ACCEPTED=True
kubectl -n enzarb-system get gateway enzarb                         # PROGRAMMED=True
kubectl -n enzarb-system get certificate enzarb-tls                 # READY=True
curl -k --resolve enzarb.local:443:127.0.0.1 https://enzarb.local/  # 404 from Envoy until routes exist
kubectl -n agent-sandbox-system get deploy agent-sandbox-controller
kubectl get crd sandboxes.agents.x-k8s.io
kubectl -n capsule-system get deploy capsule capsule-proxy
kubectl get crd tenants.capsule.clastix.io
kubectl get capsuleconfiguration default
kubectl -n pgop-system get deploy pgop
kubectl get crd clusters.pgop.ruck.io roles.pgop.ruck.io databases.pgop.ruck.io
```

## What the Helm chart can assume

- Gateway `enzarb` in `enzarb-system` (class `enzarb`), listeners `http` :80 and `https` :443
  (TLS secret `enzarb-tls`), routes allowed from all namespaces. HTTPRoutes attach with
  `parentRefs: [{name: enzarb, namespace: enzarb-system}]` (contracts.md section 8).
- CRDs: `sandboxes.agents.x-k8s.io` (v1beta1), `tenants.capsule.clastix.io`, and
  `clusters|roles|databases.pgop.ruck.io` (v1alpha1). Capsule's `CapsuleConfiguration` is `default`.
- capsule-proxy is Service `capsule-proxy` in `capsule-system`.

## gVisor in the dev cluster

Sandboxes with Docker (ADR-0012) must run under RuntimeClass `gvisor`. kind nodes need gVisor installed
into their containerd; do this after `kind create cluster`:

```sh
hack/kind-install-gvisor.sh enzarb      # cluster name; default `enzarb`
k -k overlays/dev                        # includes RuntimeClass gvisor
```

For each kind node container the script downloads the pinned release once on the host (cached in
`~/.cache/enzarb/gvisor/<version>`, sha512-checked against `hack/versions.env`), `docker cp`s `runsc`,
`containerd-shim-runsc-v1` and `gvisor-bin/` into `/usr/local/bin`, writes `/etc/containerd/runsc.toml`
(`net-raw = "true"` for Docker-in-gVisor networking) and appends the `runsc` handler to containerd's
`config.toml` (kind ships `version = 2`; the script refuses other versions), restarts containerd only if
something changed, and labels the node `enzarb.dev/runtime-gvisor=true`. Re-running is a no-op.
Smoke test: a pod with `runtimeClassName: gvisor` running `dmesg` shows gVisor's synthetic kernel log.

## Production overlay

`overlays/prod` is a skeleton. Sandbox isolation is gVisor only (a Kata option may be added later).
`base/gvisor-installer` is a privileged DaemonSet (namespace `gvisor-installer`) for real nodes: label the
nodes that should run sandboxes `enzarb.dev/gvisor-install=true` and it installs the pinned `runsc` and
shim into the host's `/usr/local/bin`, registers the `runsc` handler in `/etc/containerd/config.toml`
(version 2 or 3 layout), restarts containerd if needed and sets `enzarb.dev/runtime-gvisor=true`, which the
RuntimeClass selects on. gVisor publishes no generic Kubernetes installer (only GKE Sandbox, minikube addon and
the containerd quick start), hence this DaemonSet. It runs the image `ghcr.io/enzarb/enzarb-gvisor-installer`
(`installer/Dockerfile`), which bakes in the sha512-verified gVisor bundle, `hack/runsc.toml` and `install.sh`,
so nodes need no outbound access to github.com. The image is built by CI on every PR and pushed on main/`v*`.
The kustomization references it by tag until pinned: after the first push to main run
`hack/pin-installer-digest.sh` (resolves `:main` to a digest and rewrites the `images:` entry) and commit; repeat
after every gVisor bump. Needs nodes with a writable `/usr/local/bin` and systemd-managed containerd.
**Read-only-root node OSes (COS, Flatcar, Bottlerocket) are not supported by the installer**: `/usr` cannot be
written, so bake gVisor (`runsc`, `containerd-shim-runsc-v1`, `gvisor-bin/`, `runsc.toml`, the containerd
`runsc` handler) into the node image instead and just set the label `enzarb.dev/runtime-gvisor=true`.
Managed sandboxing (GKE Sandbox) likewise provisions gVisor itself.
gVisor needs no KVM (default `systrap` platform), so ordinary cloud VMs work.
To finish the overlay, replace the self-signed issuer (see the TODO in the kustomization), set the Envoy
Service type or annotations for your load balancer, and add HA/resource patches for Capsule and pgop.

## CI

`.github/workflows/ci.yml` runs `hack/validate.sh` (`kubectl kustomize` on every overlay and
staged base, then `kubeconform -strict` with the datree CRDs-catalog, skipping kinds without a
schema) and checks the vendored bases are reproducible. Nothing is applied to a cluster.
