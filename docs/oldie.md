# Single-node install on oldie (RKE2)

`mise run oldie:up` brings enzarb up on one server (`oldie`, 192.168.33.11) at `https://enzarb.dev`. Every step is idempotent: re-running converges and reports no change. Desired state is declared in files, not in the scripts:

| Layer | Declared in | Applied by | Converges via |
|---|---|---|---|
| Host: kernel settings, LVM volume group, gVisor, RKE2 and its config, registry auth, local kubeconfig, Cloudflare Tunnel (tunnel, ingress, token Secret, DNS) | `enzarb-kubernetes/rke2/` (Ansible role, inventory, pins in `hack/versions.env`) | `mise run oldie:host` | Ansible (`changed=0` on re-run; `-- --check --diff` shows drift) |
| Cluster prerequisites: cert-manager, Envoy Gateway, Capsule, pgop, agent-sandbox, OpenEBS LVM, issuers, StorageClass | `enzarb-kubernetes/overlays/oldie` | `mise run oldie:cluster` | `kubectl apply --server-side` |
| Secrets | `.env.oldie` (not committed) | `mise run oldie:secrets` | `kubectl apply` of a generated manifest |
| enzarb | `enzarb-helm/enzarb` (`values.yaml` pins the release version of every component, `values-oldie.yaml`) | `mise run oldie:helm-deploy` | `helm upgrade --install` |

## What oldie looks like

- RKE2 single server (`cni: canal`, so NetworkPolicy is enforced), ingress-nginx and RKE2's own snapshot controller disabled (OpenEBS ships its own). RKE2's ServiceLB (klipper-lb) is enabled, so the Envoy Gateway `LoadBalancer` Service gets the node address and binds host ports 80 and 443 (a host port is only granted if it is free, so it cannot shadow etcd or the API server the way a widened NodePort range could).
- **gVisor** (`runsc`) installed on the host from the pinned, sha512-checked release. RKE2's containerd template registers the `runsc` handler and the node carries `enzarb.dev/runtime-gvisor=true`, so RuntimeClass `gvisor` (sandboxes, ADR-0012) works. No installer DaemonSet.
- **Storage** is `/dev/sdb` (5.5 TB, the former Rook disk) as LVM volume group `enzarb`, owned by nothing else. OpenEBS LVM LocalPV provisions one xfs logical volume per PVC (StorageClass `enzarb-lvm`, default): the size is exactly what the claim requests and is enforced by the block layer, and volumes can be expanded (`allowVolumeExpansion`). They are thick, so the sum of all PVC requests must fit in the 5.46 TB; OpenEBS's thin mode creates and deletes its own pool around the first and last volume and is sized to its volumes, so it buys no overcommit here. Postgres keeps `Retain`; other PVCs are `Delete`d with their claim.
- **TLS** is Let's Encrypt for `enzarb.dev` (account `support@enzarb.dev`) through cert-manager DNS-01 on Cloudflare. DNS-01 means issuance needs no inbound connection. Visitors see Cloudflare's edge certificate; the tunnel connects to the Gateway over TLS and verifies this one.
- **Images** are pulled from `ghcr.io/enzarb/*` at the immutable release tags in `values.yaml` (`IfNotPresent`). Pull credentials are written to the node's `registries.yaml`, so they cover sandbox pods in every Project namespace too (the operator does not propagate `imagePullSecrets`).

## Prerequisites

0. **Tooling**: `mise install` provides ansible, helm, kubectl and the pinned `cf` CLI; log in once with `cf auth login` (the tunnel role uses it).
1. **ssh**: key access as `ruckc` with passwordless sudo on oldie (`rke2/inventory/oldie.yml`).
2. **Public access through a Cloudflare Tunnel** (no inbound ports, so NAT and pinhole rules do not matter). `mise run oldie:host` converges it with the `cloudflare_tunnel` Ansible role, using your `cf` CLI login on this machine (`cf auth login`; the login needs Tunnel and DNS edit on the account):
   - creates the tunnel `enzarb-oldie` (remotely managed) if missing;
   - applies its ingress rules from `enzarb-kubernetes/overlays/oldie/tunnel-config.json` (the single source for the hostname `enzarb.dev` and the origin `https://enzarb-gateway.envoy-gateway-system.svc:443`, verified against the Let's Encrypt certificate);
   - stores the connector token as Secret `cloudflared/cloudflared-tunnel`;
   - replaces the apex `A` record with a proxied `CNAME` to `<tunnel-id>.cfargotunnel.com` (mail `MX`/`TXT` records and other names are untouched).
   The `cloudflared` Deployment (2 replicas, `enzarb-kubernetes/base/cloudflared`) is applied by `oldie:cluster`. Do not forward any ports on the router. `mise run oldie:host -- --check --diff --tags tunnel` reports drift without changing anything. To opt out, set `cloudflare_tunnel_enabled: false` in the inventory.
   LAN clients can also reach the Gateway directly through ServiceLB on `192.168.33.11` (`enzarb.dev` -> `192.168.33.11` by split DNS or a hosts entry).
3. **Cloudflare token** (automated): create one *bootstrap* token in the dashboard with `Zone > Zone > Read` on the `enzarb.dev` zone plus either `User > API Tokens > Edit` (a user token: My Profile, API Tokens) or `Account API Tokens > Edit` (an account-owned token: Manage Account, Account API Tokens). The task detects which kind it is. Then:
   ```sh
   CLOUDFLARE_BOOTSTRAP_TOKEN=... mise run oldie:cloudflare-token     # or omit the variable to be prompted
   ```
   It mints `enzarb-oldie-dns-01`, restricted to the `enzarb.dev` zone with DNS Write and Zone Read only, and stores it as `cert-manager/cloudflare-api-token`. The bootstrap token is used once and never written to disk; revoke it afterwards. The task is idempotent: it keeps a token that is active and can edit the zone's DNS, and `-- --rotate` mints a new one (created and verified before the old one is deleted, so a failed rotation changes nothing). `oldie:up` runs it when `CLOUDFLARE_BOOTSTRAP_TOKEN` is set. Cloudflare cannot mint the first credential itself, so this one manual token is the floor. To skip automation, put a ready-made token in `.env.oldie` as `CLOUDFLARE_API_TOKEN`; `oldie:secrets` applies it.
4. **GitHub App** (one App for webhooks, bot access and UI login). Run `mise run oldie:init-env` to create `.env.oldie` and generate the webhook secret, then register the App (Settings, Developer settings, GitHub Apps, New GitHub App, or the manifest flow in [dev.md](dev.md)) with:
   - Webhook: active, URL `https://enzarb.dev/hooks/github`, secret = `GITHUB_WEBHOOK_SECRET` from `.env.oldie`. (The manifest flow lets GitHub generate the secret instead; copy `webhook_secret` from its response into `.env.oldie`.)
   - Callback URL: `https://enzarb.dev/login/github/callback`. Leave "Request user authorization (OAuth) during installation" off.
   - Setup URL: `https://enzarb.dev/login/github?refresh=1`, and enable "Redirect on update". After installing (or changing) the App, GitHub sends the user there, which signs them in again and refreshes the list of repositories enzarb can use. Without it, users click "Refresh repositories" in enzarb after installing.
   - Repository permissions: Contents: read and write, Checks: read and write, Issues: read and write, Pull requests: read and write, Metadata: read. No account or organization permissions.
   - Events as in [github-app.yml](github-app.yml): issues, issue_comment, pull_request, pull_request_review, pull_request_review_comment, installation, installation_repositories.
   - Users (and organizations) install the App at `https://github.com/apps/enzarb/installations/new` (replace `enzarb` with the App's slug and set `githubApp.slug` to match if it differs; enzarb shows this link when a user has no accessible installation).
   - Name it `enzarb` if available (the chart's bot login defaults to `enzarb[bot]`; otherwise set `connectors.botLogin`/`workflows.botLogin`).
   - Afterwards: note the App ID, generate and download a private key, and generate a client secret. Fill `.env.oldie`: `GITHUB_APP_ID`, `GITHUB_APP_PRIVATE_KEY` (path to the .pem), `GITHUB_OAUTH_CLIENT_ID` (the App's *Client ID*), `GITHUB_OAUTH_CLIENT_SECRET`.
5. **GHCR**: a GitHub user and a token with `read:packages` (`GHCR_USER`, `GHCR_TOKEN`).
6. **Releases**: the component images must be released (see [ci.md](ci.md#releases)); `values.yaml` pins them.

```sh
mise run oldie:init-env             # creates .env.oldie (mode 600) and the webhook secret; then fill in the rest
mise install                        # ansible-core, helm, kubectl, ...
mise run oldie:up
```

Or step by step: `oldie:host` (also converges the tunnel), `oldie:cloudflare-token` (once, or to rotate), `oldie:secrets`, `oldie:cluster`, `oldie:helm-deploy`. `oldie:cluster` also runs probes: a pod on RuntimeClass `gvisor` must show gVisor's `dmesg`, a deny-all NetworkPolicy must actually block traffic, and a PVC must mount into a gVisor pod. Set `ENZARB_OLDIE_SKIP_CERT=1` to skip the Gateway/Certificate wait until the Cloudflare token exists.

The kubeconfig is `~/.kube/enzarb-oldie.yaml` (context `enzarb-oldie`); it never touches your default kubeconfig.

## Upgrading

- **enzarb**: a component release updates the chart's pinned version automatically ([ci.md](ci.md#releases)). Pull the chart repo and run `mise run oldie:helm-deploy`. To deploy from the published chart instead of the checkout, set `ENZARB_CHART_VERSION` in `.env.oldie`.
- **RKE2, gVisor, add-ons**: change the pin in `enzarb-kubernetes/hack/versions.env` (and the URL for remote bases), then re-run `oldie:host` / `oldie:cluster`.
- **Drift check**: `mise run oldie:host -- --check --diff`.

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
- **Host converge fails on the disk**: while the volume group does not exist, the role refuses a disk that carries any filesystem or partition table (it never wipes one; `lvg` alone would, so the role checks first). Clear the disk deliberately (`wipefs -a`), then re-run.
- **gVisor probe fails**: `grep runsc /var/lib/rancher/rke2/agent/etc/containerd/config.toml` on oldie; the template is `config-v3.toml.tmpl` in the role.
