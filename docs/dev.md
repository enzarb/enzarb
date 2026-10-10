# Local development on kind

One command, `mise run dev:helm-deploy`, builds the images and brings enzarb up on a local kind cluster. This page covers the prerequisites, the GitHub setup it needs, and a first end-to-end run.

The tasks are in `.mise/tasks/dev/`:

| Task | What it does |
|---|---|
| `mise run dev:cluster` | Creates kind cluster `enzarb` (from `enzarb-kubernetes/kind/cluster.yaml`) if missing, installs gVisor into the nodes (`hack/kind-install-gvisor.sh`) and checks it with a `dmesg` probe pod. Applies `enzarb-kubernetes` in the staged order with `kubectl wait`, adds `system:serviceaccounts:enzarb` to Capsule's `users`, and runs a NetworkPolicy enforcement probe. |
| `mise run dev:images [component...]` | Builds the images as `:dev` with the right build secrets and `kind load`s them. Components: `ui sandbox-proxy sandbox operator receiver workflows`. No argument builds all six. |
| `mise run dev:secrets` | Creates namespace `enzarb` and Secret `enzarb-github` from `.env.dev`. |
| `mise run dev:helm-deploy` | `dev:cluster`, `dev:images`, `dev:secrets`, then `helm upgrade --install enzarb enzarb-helm/enzarb -f values-dev.yaml --set secrets.github.existingSecret=enzarb-github --wait`. Prints a checklist. |
| `mise run dev:webhook-relay` | Relays smee.io deliveries to `https://enzarb.local/hooks/github`. |
| `mise run dev:down` | Deletes the kind cluster. |

## Prerequisites

- Docker with BuildKit (the builds use `--secret`), and `mise` (`mise install` provides `kind`, `kubectl`, `helm`, `gh`, `node`).
- `gh auth login` with the `read:packages` scope (`gh auth refresh -s read:packages`). `dev:images` passes `gh auth token` to the UI build as `NODE_AUTH_TOKEN` to install `@enzarb/proto` from GitHub Packages.
- An ssh key with read access to the private `enzarb` repos. `dev:images` uses `ENZARB_DEV_SSH_KEY` (default `~/.ssh/id_ed25519`) for all three build secrets (`proto_key`, `crds_key`, `connectors_key`). It is removed from the image layers by the Dockerfiles.
- All component repos cloned: `mise run repos:init`.
- Ports 80 and 443 free on the host (kind maps them to the Envoy Gateway).
- Host name entry:
  ```sh
  echo '127.0.0.1 enzarb.local' | sudo tee -a /etc/hosts
  ```
- The Gateway certificate is self-signed (cert-manager). Browsers warn; either accept the warning or trust the CA:
  ```sh
  kubectl --context kind-enzarb -n enzarb-system get secret enzarb-tls -o jsonpath='{.data.ca\.crt}' | base64 -d > enzarb-ca.crt
  ```
  and import `enzarb-ca.crt` into your browser or OS trust store. Use `curl -k` otherwise.

## Create the GitHub App

enzarb needs one **GitHub App**: webhooks, API access as the bot, clone tokens, and UI login (user authorization).

1. Create a smee channel at <https://smee.io/new> and put its URL in `.env.dev` as `SMEE_URL` (copy `.env.dev.example` first).
2. Edit a copy of the manifest so `hook_attributes.url` is your smee URL. The permissions and events are in [github-app.yml](github-app.yml) (contracts.md section 10). Convert it to JSON:
   ```sh
   npx --yes js-yaml docs/github-app.yml > /tmp/app.json
   ```
3. Register it through the manifest flow. Save this as a local `register.html`, paste the JSON into the textarea, and open the file in a browser where you are logged in to GitHub (for an organization, change the action to `https://github.com/organizations/<org>/settings/apps/new`):
   ```html
   <form method="post" action="https://github.com/settings/apps/new">
     <textarea name="manifest" rows="30" cols="100"></textarea>
     <button>Create GitHub App</button>
   </form>
   ```
4. Confirm the name on GitHub. GitHub then redirects to `http://localhost:3000/github-app-created?code=<CODE>`. The page will not load; copy `<CODE>` from the address bar. It expires in one hour.
5. Exchange the code for the credentials:
   ```sh
   gh api -X POST /app-manifests/<CODE>/conversions > /tmp/app-conversion.json
   jq -r .pem /tmp/app-conversion.json > ~/.config/enzarb/app-private-key.pem && chmod 600 ~/.config/enzarb/app-private-key.pem
   jq '{id, slug, webhook_secret}' /tmp/app-conversion.json
   ```
6. Fill `.env.dev`: `GITHUB_APP_ID` (the `id`), `GITHUB_APP_PRIVATE_KEY` (the path to the pem file), `GITHUB_WEBHOOK_SECRET` (the `webhook_secret`). Delete `/tmp/app.json` and `/tmp/app-conversion.json`.
7. Note the App's slug. Its bot login is `<slug>[bot]`; the chart defaults to `enzarb[bot]` and handle `enzarb`. If your App has another name, set `connectors.botLogin`, `workflows.botLogin` and the handles via `--set` in `dev:helm-deploy` (or rename the App to `enzarb`). Otherwise the bot's own comments are not recognised and can trigger loops.

## Sign in with GitHub (UI login)

The same GitHub App provides UI login; there is no separate OAuth App. The manifest above already lists the callback URLs (`https://enzarb.local/login/github/callback` for kind). In the App's settings, open "Client secrets", generate one, and set `GITHUB_OAUTH_CLIENT_ID` (the App's Client ID, not its App ID) and `GITHUB_OAUTH_CLIENT_SECRET` in `.env.dev`. If you registered the App without `callback_urls`, add the URL under "Callback URL" in its settings. A private (`public: false`) App may only allow its owner to sign in; if other users get an authorization error, make the App public.

## First deploy

```sh
cp .env.dev.example .env.dev   # fill it in as above
mise run dev:helm-deploy
```

The first run takes a while (cluster, six image builds, pgop provisioning Postgres, three migration Jobs in order). It ends with a checklist; fix anything that is not Running or Completed (see Troubleshooting). Iterate on one component with `mise run dev:images workflows` followed by `kubectl --context kind-enzarb -n enzarb rollout restart deploy/<name>`.

In a second terminal keep the relay running:

```sh
mise run dev:webhook-relay
```

Open <https://enzarb.local> and log in with GitHub.

## Create the first Project

1. In the UI, create a Project (name `enzarb`, owner your GitHub login) with the repository `https://github.com/enzarb/enzarb` (add more repos as needed).
2. Watch the boot checklist: TenantReady, NamespaceReady, SandboxReady, Ready. The operator creates namespace `enzarb-p-<name>` through Capsule and starts the sandbox.
3. Open the Project's terminal and run the guided Claude login once:
   ```
   claude setup-token
   ```
   Complete the browser step, paste the code back. The sandbox captures the `sk-ant-oat01-` token (the UI shows the credential as configured). Agent runs fail until this is done.

## Sandboxes run under gVisor, with Docker

gVisor is installed automatically: `dev:cluster` runs `enzarb-kubernetes/hack/kind-install-gvisor.sh enzarb` after the cluster exists and before the overlays (RuntimeClass `gvisor` comes from `overlays/dev`). It then starts a throwaway pod with `runtimeClassName: gvisor` and fails unless its `dmesg` shows gVisor.

Every sandbox runs under the operator's `--sandbox-runtime-class` (`gvisor` in dev) and always has a Docker sidecar (ADR-0012). There is no per-Project Docker or runtime option. Check it from any Project's terminal:

```sh
docker run --rm hello-world
dmesg | head -3     # gVisor's synthetic kernel log
```

The first run pulls the image through the sandbox's NetworkPolicy (443 and DNS only).

> The chart sets the operator's runtime class (`operator.sandboxRuntimeClass: gvisor`) and, in `values-dev.yaml`, a 5Gi `docker` PVC per Project. Every Project has two PVCs (`workspace` and `docker`).

## First dogfood

1. Install the GitHub App on an enzarb repo (App settings, Install App, choose the repo). The `installation` webhook should arrive in the relay output and be answered `2xx`.
2. Open an issue on that repo. Watch the work item in the UI go triage, then plan. The bot comments on the issue (questions, or a plan).
3. Reply with `/approve` on its own line (or add the label `enzarb:approved`). The implement step runs in the sandbox, pushes `enzarb/issue-<n>` and opens a PR.
4. Review comments on the PR trigger review and address-comments steps. `@enzarb <request>` in a comment starts an ad-hoc task.

## Configuration notes

- `ENZARB_DEV_CNI=calico mise run dev:cluster` creates the cluster with Calico instead of kindnet (see below). Other overrides: `ENZARB_DEV_CLUSTER` (cluster name), `ENZARB_DEV_CALICO_VERSION`, `ENZARB_DEV_HOOK_TARGET` (relay target).
- **CNI decision.** The default is **kindnet**. kind v0.32's node networking uses kindnetd `v20260528`, and recent kindnetd releases implement `NetworkPolicy` (via kube-network-policies), so no extra CNI is installed. Because that is version-dependent, `dev:cluster` always runs a functional probe: it creates a pod, applies a deny-all ingress policy and fails the task if the pod is still reachable. If it fails, run `mise run dev:down` and then `ENZARB_DEV_CNI=calico mise run dev:cluster`, which adds `networking.disableDefaultCNI: true` to a temporary copy of the kind config (the `enzarb-kubernetes` repo is untouched) and installs Calico. The operator relies on enforcement: only the proxy and workflows executor may reach a sandbox on port 7070.

## Troubleshooting

- **Image pull errors (`ErrImagePull`, `ImagePullBackOff`).** The images are private on GHCR and need no pull secret in kind: `values-dev.yaml` sets `imagePullPolicy: Never` and the `:dev` tags are `kind load`ed. If a pod pulls anyway, the image was not loaded (rerun `dev:images <component>`) or the pod uses a non-dev tag. The sandbox image is `ghcr.io/enzarb/enzarb-sandbox:dev`; Projects with an explicit `sandbox.image` need their own load. A real cluster needs `imagePullSecrets`.
- **Database login failures after pgop creates roles.** pgop generates passwords, and the chart builds `DATABASE_URL` from the credential Secret with `$(VAR)` expansion. If a password contains URL-reserved characters (`@ : / ? # %`) the URL is parsed wrongly and sqlx or node-postgres fail to connect or authenticate. Check `kubectl -n enzarb get secret enzarb-pg-workflows-credentials -o jsonpath='{.data.password}' | base64 -d`; if it has such a character, delete the Secret/Role so pgop regenerates it, or file a chart issue to percent-encode.
- **Websocket (terminal or run log) drops after a while.** Idle connections are cut by the proxy (`PROXY_IDLE_TIMEOUT_SECS`, default 900, chart value `sandboxProxy.idleTimeoutSecs`), by Envoy (`BackendTrafficPolicy` connection idle timeout and the `/sandbox/` route request timeout in the chart's `route.*` values) and by any browser extension or VPN in front. Raise the chart values consistently. The UI reconnects, but long-lived idle sessions need all three aligned.
- **Project stuck at TenantReady or namespace creation denied.** Capsule only acts for requests from its configured `users`. The operator and UI run as service accounts in namespace `enzarb`, so `system:serviceaccounts:enzarb` must be in `CapsuleConfiguration/default` `spec.users`. `dev:cluster` adds it; verify with `kubectl get capsuleconfiguration default -o jsonpath='{.spec.users}'`. A recreated or re-applied `enzarb-kubernetes` overlay resets it; rerun `dev:cluster`.
- **Sandboxes are reachable from where they should not be.** The CNI does not enforce NetworkPolicy. Rerun `dev:cluster` (it probes) or switch to Calico as described above.
- **Webhooks never arrive.** Check the smee channel URL matches the App's webhook URL, that `dev:webhook-relay` is running, and App settings, Advanced, Recent deliveries. A `401` from the receiver means `GITHUB_WEBHOOK_SECRET` does not match the App's secret; re-run `dev:secrets` and restart the receiver.
- **Browser says "Cross-site POST form submissions are forbidden" (UI).** The Gateway must forward `X-Forwarded-Proto` and the UI needs `ENZARB_PUBLIC_URL=https://enzarb.local`; the chart sets both. Use `https://`, not `http://`.
- **Docker build cannot fetch private dependencies.** `ENZARB_DEV_SSH_KEY` must be a key with read access to `enzarb-proto`, `enzarb-sandbox-operator` and `enzarb-connectors`, and `gh auth token` needs `read:packages` for the UI build.
- **gVisor: `dev:cluster` fails the probe or sandbox pods stay Pending/ContainerCreating.** Check `kubectl --context kind-enzarb get runtimeclass gvisor` and `kubectl --context kind-enzarb get nodes -L enzarb.dev/runtime-gvisor` (the label must be `true`; rerun `dev:cluster`, the installer is idempotent). `docker exec enzarb-control-plane runsc --version` and `grep runsc /etc/containerd/config.toml` inside the node show whether the install took. `kubectl describe pod` shows runtime errors such as `no runtime for "runsc"`.
- **gVisor platform and kernel features.** gVisor uses the `systrap` platform, which needs no KVM, so it works in kind and on WSL2 or cloud VMs. It does need ptrace/seccomp and user namespaces on the host kernel; an old or locked-down kernel (or a rootless Docker host with restricted seccomp) can stop `runsc` starting. The sidecar's networking relies on runsc `net-raw = "true"` (and `allow-packet-socket-write` for Docker 28+); the installer sets `net-raw`. If `docker run` fails with network errors, check `/etc/containerd/runsc.toml` in the node.
- **Docker-in-gVisor limitations.** `iptables` is disabled, so `docker run -p` publishes nothing (reach containers by IP or use `--network=host`); storage uses `vfs` (no copy-on-write, so images use more disk and builds are slower); some syscalls and filesystems are unsupported. See [gVisor in the dev cluster](../enzarb-kubernetes/README.md#gvisor-in-the-dev-cluster) and [Docker in the sandbox](../enzarb-sandbox-operator/README.md#docker-in-the-sandbox-adr-0012).
- **Start over.** `mise run dev:down`, then `mise run dev:helm-deploy`.
