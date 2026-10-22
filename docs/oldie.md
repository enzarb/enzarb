# Single-node install on oldie (RKE2)

`mise run oldie:up` brings enzarb up on one server (`oldie`, 192.168.33.11) at `https://enzarb.dev`. Every step is idempotent: re-running converges and reports no change. Desired state is declared in files, not in the scripts:

| Layer | Declared in | Applied by | Converges via |
|---|---|---|---|
| Host: kernel settings, LVM volume group, gVisor, RKE2 and its config, registry auth, local kubeconfig, Cloudflare Tunnel (tunnel, ingress, token Secret, DNS) | `kubernetes/rke2/` (Ansible role, inventory, pins in `hack/versions.env`) | `mise run oldie:host` | Ansible (`changed=0` on re-run; `-- --check --diff` shows drift) |
| Cluster prerequisites: cert-manager, Envoy Gateway, Capsule, pgop, agent-sandbox, OpenEBS LVM, issuers, StorageClass | `kubernetes/overlays/oldie` | `mise run oldie:cluster` | `kubectl apply --server-side` |
| Secrets | `.env.oldie` (not committed) | `mise run oldie:secrets` | `kubectl apply` of a generated manifest |
| enzarb | `helm/enzarb` (`values.yaml` pins every component, `values-oldie.yaml` is embedded in the HelmChart) | **bootstrap once**: `mise run oldie:bootstrap-helmchart`; **upgrades**: `enzarb-updater` | RKE2 helm-controller, driven by the `HelmChart` CR |

## What oldie looks like

- RKE2 single server (`cni: canal`, so NetworkPolicy is enforced), ingress-nginx and RKE2's own snapshot controller disabled (OpenEBS ships its own). RKE2's ServiceLB (klipper-lb) is enabled, so the Envoy Gateway `LoadBalancer` Service gets the node address and binds host ports 80 and 443 (a host port is only granted if it is free, so it cannot shadow etcd or the API server the way a widened NodePort range could).
- **gVisor** (`runsc`) installed on the host from the pinned, sha512-checked release. RKE2's containerd template registers the `runsc` handler and the node carries `enzarb.dev/runtime-gvisor=true`, so RuntimeClass `gvisor` (sandboxes, ADR-0012) works. No installer DaemonSet.
- **Storage** is `/dev/sdb` (5.5 TB, the former Rook disk) as LVM volume group `enzarb`, owned by nothing else. OpenEBS LVM LocalPV provisions one xfs logical volume per PVC (StorageClass `enzarb-lvm`, default): the size is exactly what the claim requests and is enforced by the block layer, and volumes can be expanded (`allowVolumeExpansion`). They are thick, so the sum of all PVC requests must fit in the 5.46 TB; OpenEBS's thin mode creates and deletes its own pool around the first and last volume and is sized to its volumes, so it buys no overcommit here. Postgres keeps `Retain`; other PVCs are `Delete`d with their claim.
- **TLS** is Let's Encrypt for `enzarb.dev` (account `support@enzarb.dev`) through cert-manager DNS-01 on Cloudflare. DNS-01 means issuance needs no inbound connection. Visitors see Cloudflare's edge certificate; the tunnel connects to the Gateway over TLS and verifies this one.
- **Images** are pulled from `ghcr.io/enzarb/*` at the immutable release tags in `values.yaml` (`IfNotPresent`). Pull credentials are written to the node's `registries.yaml`, so they cover the sandbox pods in the shared sandbox namespace too (the operator does not propagate `imagePullSecrets`).

## Prerequisites

0. **Tooling**: `mise install` provides helm, kubectl and the pinned `cf` CLI, and the oldie tasks run with `MISE_ENV=ansible` themselves, which adds the ansible pins from `mise.ansible.toml`; log in once with `cf auth login` (the tunnel role uses it).
1. **ssh**: key access as `ruckc` with passwordless sudo on oldie (`rke2/inventory/oldie.yml`).
2. **Public access through a Cloudflare Tunnel** (no inbound ports, so NAT and pinhole rules do not matter). `mise run oldie:host` converges it with the `cloudflare_tunnel` Ansible role, using your `cf` CLI login on this machine (`cf auth login`; the login needs Tunnel and DNS edit on the account):
   - creates the tunnel `enzarb-oldie` (remotely managed) if missing;
   - applies its ingress rules from `kubernetes/overlays/oldie/tunnel-config.json` (the single source for the hostname `enzarb.dev` and the origin `https://enzarb-gateway.envoy-gateway-system.svc:443`, verified against the Let's Encrypt certificate);
   - stores the connector token as Secret `cloudflared/cloudflared-tunnel`;
   - replaces the apex `A` record with a proxied `CNAME` to `<tunnel-id>.cfargotunnel.com` (mail `MX`/`TXT` records and other names are untouched).
   The `cloudflared` Deployment (2 replicas, `kubernetes/base/cloudflared`) is applied by `oldie:cluster`. Do not forward any ports on the router. `mise run oldie:host -- --check --diff --tags tunnel` reports drift without changing anything. To opt out, set `cloudflare_tunnel_enabled: false` in the inventory.
   LAN clients can also reach the Gateway directly through ServiceLB on `192.168.33.11` (`enzarb.dev` -> `192.168.33.11` by split DNS or a hosts entry).
3. **Cloudflare token** (automated): create one *bootstrap* token in the dashboard with `Zone > Zone > Read` on the `enzarb.dev` zone plus either `User > API Tokens > Edit` (a user token: My Profile, API Tokens) or `Account API Tokens > Edit` (an account-owned token: Manage Account, Account API Tokens). The task detects which kind it is. Then:
   ```sh
   CLOUDFLARE_BOOTSTRAP_TOKEN=... mise run oldie:cloudflare-token     # or omit the variable to be prompted
   ```
   It mints `enzarb-oldie-dns-01`, restricted to the `enzarb.dev` zone with DNS Write and Zone Read only, and stores it as `cert-manager/cloudflare-api-token`. The bootstrap token is used once and never written to disk; revoke it afterwards. The task is idempotent: it keeps a token that is active and can edit the zone's DNS, and `-- --rotate` mints a new one (created and verified before the old one is deleted, so a failed rotation changes nothing). `oldie:up` runs it when `CLOUDFLARE_BOOTSTRAP_TOKEN` is set. Cloudflare cannot mint the first credential itself, so this one manual token is the floor. To skip automation, put a ready-made token in `.env.oldie` as `CLOUDFLARE_API_TOKEN`; `oldie:secrets` applies it.
4. **GitHub App** (one App for webhooks, API access and UI login). Run `mise run oldie:init-env` to create `.env.oldie` and generate the webhook secret, then register the App (Settings, Developer settings, GitHub Apps, New GitHub App, or the manifest flow in [dev.md](dev.md)) with:
   - Webhook: active, URL `https://enzarb.dev/hooks/github`, secret = `GITHUB_WEBHOOK_SECRET` from `.env.oldie`. (The manifest flow lets GitHub generate the secret instead; copy `webhook_secret` from its response into `.env.oldie`.)
   - Callback URL: `https://enzarb.dev/login/github/callback`. Leave "Request user authorization (OAuth) during installation" off.
   - Setup URL: `https://enzarb.dev/login/github?refresh=1`, and enable "Redirect on update". After installing (or changing) the App, GitHub sends the user back to enzarb. The repository list is read live through the App (cached about a minute); "Refresh repositories" in enzarb shows a change right away.
   - Repository permissions: Contents: read and write, Checks: read and write, Actions: read, Commit statuses: read, Issues: read and write, Pull requests: read and write, Metadata: read. No account or organization permissions.
   - Events as in [github-app.yml](github-app.yml): issues, issue_comment, pull_request, pull_request_review, pull_request_review_comment, check_suite, check_run, status, installation, installation_repositories.
   - Users (and organizations) install the App at `https://github.com/apps/enzarb/installations/new` (replace `enzarb` with the App's slug and set `githubApp.slug` to match if it differs; enzarb shows this link when a user has no accessible installation).
   - Name it `enzarb` if available (its slug decides the install link; otherwise set `githubApp.slug`).
   - Afterwards: note the App ID, generate and download a private key, and generate a client secret. Fill `.env.oldie`: `GITHUB_APP_ID`, `GITHUB_APP_PRIVATE_KEY` (path to the .pem), `GITHUB_OAUTH_CLIENT_ID` (the App's *Client ID*), `GITHUB_OAUTH_CLIENT_SECRET`.
5. **GHCR**: a GitHub user and a token with `read:packages` (`GHCR_USER`, `GHCR_TOKEN`). One credential, three uses, all written by automation from `.env.oldie` (never inlined in values or manifests): the node's `registries.yaml` (image pulls, `oldie:host`), the Secret `kube-system/enzarb-chart-registry` (type `kubernetes.io/dockerconfigjson`, the HelmChart's `spec.dockerRegistrySecret`, so the helm-controller can pull the private OCI chart) and the Secret `enzarb/enzarb-updater-registry` (key `registry-token`, `ENZARB_UPDATER_REGISTRY_TOKEN` for the updater's tag polling). The last two come from `oldie:secrets`. The same Secret also feeds the workflows Deployment as `ENZARB_NPM_TOKEN` (`workflows.npmToken`, `optional: true`), which hands it to agent runs as `RunStart.npm_token` / `NODE_AUTH_TOKEN` so `npm ci` can read private `@enzarb/*` packages. **Trade-off:** this is a long-lived PAT, and `read:packages` covers every package its owning user can read, not only `@enzarb/*`. It is exposed only to the run process, redacted from transcripts and callbacks, and never persisted (not in `jobs.input`, `request.json`, state or `.npmrc`), but a prompt-injected agent could still use it within the run for any package read. Prefer a dedicated machine user that can read only the enzarb packages. Rotate by updating `GHCR_TOKEN` in `.env.oldie` and re-running `oldie:secrets`, then restart the workflows Deployment (the env var is read at startup).
6. **Releases**: the component images must be released (see [ci.md](ci.md#releases)); `values.yaml` pins them.

```sh
mise run oldie:init-env             # creates .env.oldie (mode 600) and the webhook secret; then fill in the rest
mise install                        # helm, kubectl, cf, ...
(cd kubernetes && mise install)     # ansible-core, ansible-lint
mise run oldie:up
```

Or step by step: `oldie:host` (also converges the tunnel), `oldie:cloudflare-token` (once, or to rotate), `oldie:secrets`, `oldie:cluster`, `oldie:bootstrap-helmchart` (once). `oldie:cluster` also runs probes: a pod on RuntimeClass `gvisor` must show gVisor's `dmesg`, a deny-all NetworkPolicy must actually block traffic, and a PVC must mount into a gVisor pod. Set `ENZARB_OLDIE_SKIP_CERT=1` to skip the Gateway/Certificate wait until the Cloudflare token exists.

The kubeconfig is `~/.kube/enzarb-oldie.yaml` (context `enzarb-oldie`); it never touches your default kubeconfig.

## Upgrading

- **enzarb: automatic, nobody runs a deploy task.** Merge, then release (Conventional Commits, [ci.md](ci.md#releases)), then chart publish (`oci://ghcr.io/enzarb/charts/enzarb`), then `enzarb-updater` polls the registry (every 5 minutes), takes the highest plain `X.Y.Z` tag and patches `spec.version` of the RKE2 `HelmChart` `kube-system/enzarb` (never down), then the helm-controller upgrades the release. There is no GitHub webhook and no webhook secret. Verify read-only: `kubectl -n kube-system get helmchart enzarb`, `kubectl -n enzarb logs deploy/enzarb-updater`, `kubectl -n kube-system get jobs | grep helm-install-enzarb`. If the pipeline is stuck, fix the pipeline; do not hand-deploy.
- **The HelmChart is created once** by `oldie:bootstrap-helmchart` (also the last step of `oldie:up`), which calls `kubernetes/hack/oldie-helmchart.sh`: it creates or adopts the object (refusing literal secrets in the values), points `spec.dockerRegistrySecret` at `enzarb-chart-registry`, embeds `values-oldie.yaml` as `valuesContent`, and never lowers `spec.version`. It is not an upgrade path. Pull the repo first (the CRDs in `helm/` come from the checkout and must not be older than the deployed chart). Re-run it (by a human) only when `values-oldie.yaml` changes, since that file is not in the published chart, or to repair a deleted HelmChart. If the HelmChart is missing the updater logs it and does nothing; it never creates one.
- **Known gap: CRDs.** Neither Helm nor the helm-controller upgrades `crds/` after the first install. A chart release that changes a CRD (the `Project` CRD, pgop) needs `oldie:bootstrap-helmchart` re-run by a human, which server-side applies the chart checkout's CRDs first, until CRDs are shipped some other way.
- **A failed upgrade** is not rolled back or alerted on yet (deferred, [architecture.md](architecture.md#self-update-chain)). The helm-controller's job log shows why: `kubectl -n kube-system logs job/helm-install-enzarb`.

- **HelmChart failure policy.** The oldie HelmChart sets `spec.failurePolicy: abort` and `spec.forceConflicts: true`. The helm-controller default (`reinstall`) runs `helm uninstall` after a failed install; that is what deleted the pgop `Cluster`/`Database`/`Role` CRs and the generated Secret in the incident below. With `abort` a failed install stays failed for a human to look at. The chart also marks those CRs and the generated Secret `helm.sh/resource-policy: keep` (enzarb-helm#32; adoption on reinstall is in its README).

### Recovery after a failed install

1. Stop. Do not delete the PVC, the `enzarb` namespace or the pgop `Cluster`. The data PV's reclaim policy is `Delete`, so removing the PVC destroys the Postgres data.
2. Snapshot first: on oldie, `sudo lvcreate -s -n pg-snap-$(date +%F) -L 10G <vg>/<pg-lv>` (find the LV with `lvs`; the PV is an OpenEBS LVM volume).
3. Read-only diagnosis: `kubectl -n kube-system get helmchart enzarb -o yaml` and the `helm-install-enzarb` job log.
4. Fix the cause (values, chart version), then re-run `mise run oldie:bootstrap-helmchart` (by a human). If kept resources exist from an earlier uninstall, see "Uninstall keeps data definitions" in the `helm/` README to adopt them.
5. Confirm the Postgres pod and PVC are still bound and the app is healthy.

### Incident summary (oldie outage)

1. An install/upgrade of the `enzarb` HelmChart failed.
2. The helm-controller's default `failurePolicy: reinstall` ran `helm uninstall`.
3. That deleted the pgop `Cluster`, `Database` and `Role` CRs and the chart-generated Secret (session JWT, callback secret, realtime JWT).
4. enzarb was down until the resources were recreated.
5. Fixes: `failurePolicy: abort` and `forceConflicts: true` (enzarb-kubernetes#32), `resource-policy: keep` in the chart (enzarb-helm#32), this procedure.
- **RKE2, gVisor, add-ons**: change the pin in `kubernetes/hack/versions.env` (and the URL for remote bases), then re-run `oldie:host` / `oldie:cluster`.
- **Drift check**: `mise run oldie:host -- --check --diff`.

### One-time human actions (cutover and bootstrap)

- [ ] `.env.oldie` has `GHCR_USER` and `GHCR_TOKEN` (`read:packages`; the same credential as the image pulls).
- [ ] After `oldie:host`: `mise run oldie:secrets` (creates `kube-system/enzarb-chart-registry` and `enzarb/enzarb-updater-registry`).
- [ ] A chart release that pins `enzarb-updater` >= 0.2.0 (polling) is published (`oci://ghcr.io/enzarb/charts/enzarb`).
- [ ] `mise run oldie:bootstrap-helmchart` once. For an existing install the HelmChart starts at the deployed Helm release's chart version and adopts the release `enzarb` in namespace `enzarb`; the updater then moves it to the newest chart. On a fresh cluster it starts at `ENZARB_CHART_VERSION` or the newest published tag. The task waits for the helm-controller's install job.
- [ ] Remove the old webhook leftovers, if any: the `/updater/hook` GitHub release webhook on the archived `enzarb/enzarb-helm` repo (not needed any more) and the generated `enzarb/enzarb-updater` webhook Secret.

## Webhooks

GitHub delivers directly to `https://enzarb.dev/hooks/github` through the tunnel; no relay is needed. Set that as the GitHub App's webhook URL (with the `GITHUB_WEBHOOK_SECRET` from `.env.oldie`) and check App settings, Advanced, Recent deliveries for `2xx`. If you proxy through Cloudflare, make sure no WAF or bot rule blocks GitHub's webhook user agent on `/hooks/`.

## Reset

`ENZARB_OLDIE_CONFIRM=oldie mise run oldie:reset` removes RKE2, all cluster state **and the volume group, with every PVC's data** (Postgres included). `oldie:up` rebuilds from scratch. The Cloudflare tunnel and DNS record are not touched (the next `oldie:host` re-stores the connector token).

## Troubleshooting

- **Storage: PVC Pending**: `kubectl -n kube-system logs ds/openebs-lvm-node -c openebs-lvm-plugin` and `lvs`/`vgs` on oldie. The StorageClass `volgroup` (`overlays/oldie/storageclass.yaml`) must equal `rke2_lvm_vg` in the inventory.
- **Tunnel**: `kubectl -n cloudflared get pods` (both Ready means connected to Cloudflare's edge), `kubectl -n cloudflared logs deploy/cloudflared`, and `cf tunnels list`. A 502/1033 from Cloudflare means no connector is connected; a 404 means the hostname is missing from `tunnel-config.json`; 502 with a healthy connector means the Gateway Service `enzarb-gateway` or its certificate is wrong.
- **Unreachable from outside** (only if you opted out of the tunnel and use port forwarding): from a host outside your network, `curl -vk https://enzarb.dev/`. Check the router forwards 80/443 to 192.168.33.11, and that `kubectl -n envoy-gateway-system get svc enzarb-gateway` has the node address as its external IP (ServiceLB). A Cloudflare 52x error usually means SSL mode is not Full (strict) or the origin is down.
- **Certificates**: `kubectl -n enzarb-system describe certificate enzarb-tls`, then `kubectl get challenges -A`. Re-run `mise run oldie:cloudflare-token` (it reports whether the stored token is active and can edit the zone) or rotate it with `-- --rotate`. Use the `letsencrypt-staging` ClusterIssuer first if you are iterating (production rate limits apply).
- **ImagePullBackOff**: `GHCR_TOKEN` was empty when `oldie:host` ran (no `registries.yaml`), the token lacks `read:packages`, or the pinned tag was never released. Re-run `oldie:host` after fixing `.env.oldie`.
- **Chart not pulled / updater not ready**: `kubectl -n kube-system get helmchart enzarb -o yaml` and the `helm-install-enzarb` job log (a 401/403 means `enzarb-chart-registry` holds a bad token: fix `.env.oldie`, `oldie:secrets`); `kubectl -n enzarb logs deploy/enzarb-updater` (the pod is not Ready while the last poll failed).
- **Host converge fails on the disk**: while the volume group does not exist, the role refuses a disk that carries any filesystem or partition table (it never wipes one; `lvg` alone would, so the role checks first). Clear the disk deliberately (`wipefs -a`), then re-run.
- **gVisor probe fails**: `grep runsc /var/lib/rancher/rke2/agent/etc/containerd/config.toml` on oldie; the template is `config-v3.toml.tmpl` in the role.
