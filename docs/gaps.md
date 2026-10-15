# Open questions and known gaps

The owning repo is in brackets. Resolve 2–5 first: they shape the CRDs, the protocol handshake and the database schemas.

1. **Repo hosting and process** [enzarb]: no git history or remotes yet. Decide hosting/naming, CI, release/versioning, and image registry. Pin toolchains: `mise.toml` uses `latest`, the sandbox uses `ubuntu:26.04`, and the Rust crates need lockfiles.
2. **AuthN/AuthZ** [ui, sandbox-proxy]:
   - the login provider;
   - the token format the UI issues and the proxy checks (JWT, signing keys);
   - mapping connector identities to enzarb users (`app_api.identity_v`).
3. **Proxy/executor → sandbox-init trust** [sandbox-container, sandbox-proxy, workflows]: mTLS or projected ServiceAccount tokens; NetworkPolicy blocking sandbox-to-sandbox traffic in the shared namespace (ADR-0013); per-Project egress policy.
4. **Secrets** [operator, connectors]:
   - where the Claude OAuth token (resolved: encrypted in Postgres, [database.md](database.md)) and the GitHub App private key live;
   - ~~whose Claude token pays for webhook-triggered runs~~ resolved: the Project owner's, one per user, encrypted in Postgres ([ADR-0004](adr/0004-claude-credentials.md) amendment 2026-10-13);
   - keeping tokens unreadable if sandboxes ever get Kubernetes API access (ADR-0013 limits it to other namespaces).
5. **Prompt injection** [workflows, connectors]: issue authors steer an agent that holds write credentials. Mitigations:
   - short-lived, single-repo clone tokens for each run;
   - only users with write access can approve;
   - the agent pushes to branches only, never to protected branches;
   - consider restricting triage on public repos to known actors.
6. **Worktree mode** [sandbox-container]: worktree slots per sandbox, cleanup, and conflicts with someone working interactively in the same sandbox. **Fresh mode** depends on agent-sandbox warm pools/claims; confirm the upstream API in v0.5.x.
7. **Container runtime in the sandbox** [sandbox-container]: resolved by ADR-0012 and ADR-0016 (rootful dockerd inside the sandbox container under gVisor; rootless is not used). Still open: CPU/memory quotas per sandbox and per run. dockerd and inner containers share the sandbox container's limits, so a heavy build can OOM the agents too. Image pruning on the docker volume is tracked separately.
8. **Proxy availability** [sandbox-proxy, proto]: websockets are stateful. Running several replicas needs reconnect and resume semantics in the protocol and timeouts on flow-control credit. Workspace persistence (PVC lifecycle, hibernate/resume) is also open.
9. **pgop for a durable engine** [kubernetes, pgop]: max 1 replica; backups, PITR and pooling aren't documented. LISTEN needs a direct session-mode connection. Confirm whether object-level grants are possible in pgop. See [database.md](database.md#known-pgop-gaps-for-this-use).
10. **Transcript storage** [workflows, sandbox-container]: which object store (S3-compatible? MinIO in dev?), retention, and access control for transcripts.
11. **Observability** [all]: tracing from connectors → workflows → sandbox, with the delivery id as the correlation id; sandbox utilization metrics (decided in [ADR-0019](adr/0019-sandbox-utilization-polling.md): workflows polls sandbox-init, stored in Postgres; open until implemented); workflow admin views for stuck items, retries and dead jobs; audit of terminal sessions; limits on Claude usage and CPU.
12. **CRD versioning** [operator]: a conversion strategy for `v1alpha1`, and status conditions designed so the UI's boot checklist maps 1:1 to them.
13. **Testing** [all]:
    - table tests for pure transitions;
    - engine tests against real Postgres (testcontainers);
    - shared `proto/` test vectors for Rust and TS;
    - operator envtest;
    - end-to-end on kind with recorded webhook fixtures, plus a smee-style relay for live GitHub.
14. **SvelteKit remote functions** [ui]: they are still behind an experimental flag (`experimental.remoteFunctions`). Pin versions. Browsers connect to `sandbox-proxy` directly for websockets.
15. **Human review before merge** [workflows]: should `ReadyToMerge` also require a human GitHub review? Current decision: no. Only enzarb's own review approval is required, and the user's Merge click is the human gate.
16. **Billing traceability for sandbox usage** [workflows, ui]: the raw samples (`workflows_api.sandbox_stats_v`) are pruned after `sandboxStatsRetentionDays` (default 90, [ADR-0019](adr/0019-sandbox-utilization-polling.md)) and capped per sandbox, so they are a monitoring window, not a billing record. A longer-term record needs its own design: durable rollups (for example hourly per project and sandbox, written by the poller before the prune), the sandbox, task or run each belongs to, a much longer or no prune horizon, and a stable versioned `workflows_api` object for it. The Metrics tab charts the short window only. Needs an ADR before implementation.
