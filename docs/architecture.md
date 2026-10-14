# enzarb architecture

enzarb runs AI coding agents in isolated Kubernetes sandboxes against code repositories. Work starts in two ways: a person in the web UI, or an event from a remote system such as a GitHub issue, PR or comment. Events drive durable workflows (triage → plan → approve → implement → review), and each agent step runs inside a sandbox.

**Scope.** Sandboxes, receiving events, and running workflows in sandboxes against repos. Deploying user environments is out of scope (see [ADR-0006](adr/0006-scope-no-environments.md)).

## Repositories

All components live in one monorepo, github.com/enzarb/enzarb ([ADR-0015](adr/0015-monorepo.md)), one top-level directory each. Each directory has its own build, checks, Dockerfile and release (tags `<dir>/vX.Y.Z`); the root holds docs, ADRs and the `mise` tasks.

| Dir | Role | Language / artifact |
|---|---|---|
| `ui/` | SvelteKit web app; remote functions are the logic layer. Login, Project management, workflow/run views, terminal client | TypeScript, container image |
| `sandbox-proxy/` | Takes the browser's single websocket, checks authorization, routes mux frames to the right sandbox's pid-1 | Rust, container image |
| `sandbox-container/` | Sandbox image plus pid-1 `sandbox-init`: supervises terminals, git worktrees and headless `claude -p` runs; captures the Claude token | Rust + Dockerfile |
| `operator/` | Reconciles `Project` (`enzarb.dev/v1alpha1`) into an agent-sandbox `Sandbox`, its auth Secret, volumes and NetworkPolicy in the shared sandbox namespace (ADR-0013); publishes the CRD types as a library crate | Rust (kube-rs), container image |
| `proto/` | Mux protocol: frame codec, credit-based flow control, `docs/protocol.md`, shared test vectors | Rust crate + npm package |
| `connectors/` | The `Connector` trait and one connector per source (GitHub first) as a library, plus the `receiver` binary that accepts webhooks | Rust library + container image |
| `workflows/` | The in-house Postgres workflow engine: migrations, state machines, reducer, executors, ticker | Rust, container image |
| `updater/` | Tiny stateless service: polls the chart registry and patches `spec.version` of the RKE2 `HelmChart` to the highest published version (never downgrades; optional release webhook) | Rust, container image |
| `realtime/` | Stateless change stream: `LISTEN work_item_changed`, pushes per-project `work_item` / `resync` frames to browsers over an authenticated websocket (`/live/v1/stream`); the UI refetches via remote functions | Rust, container image |
| `kubernetes/` | Cluster prerequisites: kind config, Envoy Gateway, agent-sandbox (v0.5.x), RuntimeClasses, pgop (Capsule is installed but unused since ADR-0013) | YAML / kustomize |
| `helm/` | Umbrella chart: all services, CRDs, HTTPRoutes, RBAC, pgop `Cluster`/`Role`/`Database`, migration Jobs | Helm |

### Library dependencies between components

```mermaid
flowchart LR
  proto[proto]
  crds[operator<br/>CRD types crate]
  conn[connectors<br/>library]
  ui[ui] --> proto
  px[sandbox-proxy] --> proto
  px --> crds
  init[sandbox-container<br/>sandbox-init] --> proto
  wf[workflows] --> proto
  wf --> conn
  wf --> crds
  rcv[connectors<br/>receiver] --> conn
  rcv --> crds
```

Dependencies are path dependencies inside the monorepo (Cargo `path = "../proto/..."`, npm `file:../proto`).

`proto/` is its own component because four programs speak it: the browser, the proxy, sandbox-init, and the workflow executor. The executor is a non-browser client that starts agent runs. See [ADR-0003](adr/0003-mux-protocol.md).

### Self-update chain

component release (`<dir>/vX.Y.Z` tag) → the in-repo chart sync bumps the image tags in `helm/` → `helm/vX.Y.Z` release → chart published to `oci://ghcr.io/enzarb/charts/enzarb` → `enzarb-updater` polls the registry's tag list (every `ENZARB_UPDATER_POLL_SECS`, default 300), picks the highest plain semver and patches `HelmChart.spec.version` (never downgrades) → the RKE2 helm-controller upgrades the release. Nobody runs a deploy task. The updater needs no inbound traffic and no webhook secret; a GitHub release webhook (`/updater/hook`) remains available but is off by default.

The `HelmChart` CR is cluster-owned: it is created once by the cluster bootstrap (`mise run oldie:bootstrap-helmchart`, [oldie.md](oldie.md#upgrading)), never by the chart or the updater, and the updater has no `create` permission. The private chart is pulled with `spec.dockerRegistrySecret`; the updater reads tags with a `read:packages` token from a Secret. `enzarb-updater` lives in the umbrella chart, so it patches the CR before the upgrade restarts it. Monitoring a failed upgrade (rollback, alerting) and upgrading CRDs are deferred. See the [updater README](https://github.com/enzarb/enzarb/tree/main/updater) for env vars and RBAC.

## Runtime view

```mermaid
flowchart LR
  B[Browser<br/>enzarb-ui client] -- HTTPS --> GW[Envoy Gateway<br/>enzarb.local]
  B -- one websocket, enzarb-proto --> GW
  GW --> UI[enzarb-ui]
  GW --> PX[sandbox-proxy]
  SRC[GitHub / later GitLab, Jira…] -- webhooks --> GW --> RCV[connectors receiver]

  subgraph DB[pgop Cluster enzarb-pg]
    PG[(database enzarb<br/>schemas: workflows, workflows_api,<br/>connectors, app)]
  end

  RCV -- ingest_event --> PG
  UI -- submit_action / read views --> PG
  PG -- NOTIFY work_item_changed --> UI

  subgraph WF[enzarb-workflows]
    RED[reducer]
    EXE[executors]
    TCK[ticker]
  end
  PG <--> RED
  PG <--> EXE
  TCK --> PG

  EXE -- Connector: comment, PR, status, reactions --> SRC
  EXE -- enzarb-proto: ensure worktree, start run --> SB
  SB -- RunFinished --> EXE

  UI -- Project CRs --> API[kube-apiserver]
  OP[sandbox-operator] -- watch / reconcile --> API
  API --> AS[agent-sandbox controller] --> SB
  PX --> SB

  subgraph T[Shared namespace enzarb-sandboxes, one sandbox per Project, no Kubernetes API access]
    SB[Sandbox pod: one container<br/>sandbox-init pid-1 as root supervises dockerd<br/>agents and git run as uid 1000<br/>git worktrees, claude -p<br/>gVisor/Kata in prod]
  end
  SB -- git --> SRC
  SB -- Claude API --> AN[Anthropic]
```

## Key flows

### 1. Create a Project
1. In the UI, the user creates a Project: linked repo remotes, sandbox template, and run mode (`worktree` or `fresh`).
2. A remote function writes the `Project` CR.
3. The operator creates the project's auth Secret, volumes, NetworkPolicy and `Sandbox` CR in the shared sandbox namespace ([ADR-0013](adr/0013-shared-sandbox-namespace.md)).
4. The UI shows a **step-by-step boot checklist** built from the Project/Sandbox status conditions. Every step is a named condition; there are no open-ended spinners.

### 2. Interactive terminal
1. The browser opens one websocket to `sandbox-proxy` with a session token issued by the UI.
2. The proxy checks the token, resolves the Project's sandbox, and dials sandbox-init.
3. Frames are multiplexed: terminals, the `claude` session, status events, file transfer. Each is a channel with credit-based flow control.

**Chat.** The Chat page uses the same socket with an `acp` channel ([ADR-0014](adr/0014-chat-tab-acp-over-mux.md), [design](design/chat-tab.md)): sandbox-init runs one `claude-agent-acp` per chat in its own worktree and relays ACP JSON-RPC through a filter; permission requests are hard-denied by policy or decided by the user, never auto-approved.

### 3. Claude credentials
The user runs a guided `claude setup-token` in the terminal of any of their sandboxes, and pid-1 captures the `sk-ant-oat01-` token. The token is per user, not per Project: pid-1 hands it to the proxy over the mux connection, and the proxy stores it encrypted in Postgres (one row per user, key from a Secret mounted only into the proxy and workflows). Workflows sends the Project owner's token with each job's `RunStart`, and the proxy hands it to the sandbox when a chat opens; the sandbox keeps no copy on disk. The token never reaches the browser or the UI. Third-party OAuth against Claude subscriptions is not allowed by Anthropic's terms ([ADR-0004](adr/0004-claude-credentials.md)).

### 4. Event-driven workflow (e.g. a new GitHub issue)
1. GitHub → `connectors receiver`: it checks the signature, normalizes the payload, calls `workflows_api.ingest_event`, and returns 200 within 10s.
2. The reducer applies the `issue` state machine. It queues a high-priority `set_progress(Received)` job (👀) and a `triage` agent job.
3. An executor starts the triage run in the Project's sandbox, in a new worktree or a fresh sandbox, over enzarb-proto. sandbox-init reports `RunFinished`, which becomes an event.
4. The workflow continues: questions → plan → `/approve` → implement → PR → review loop. Details in [workflows.md](workflows.md).

## Further reading
- [workflows.md](workflows.md): the workflow engine
- [connectors.md](connectors.md): connector contract and normalized events
- [database.md](database.md): shared Postgres, schemas and roles via pgop
- [gaps.md](gaps.md): open questions and known gaps
- [adr/](adr/): architecture decision records
