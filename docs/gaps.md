# Open questions and known gaps

The owning repo is in brackets. Resolve 2–5 first: they shape the CRDs, the protocol handshake and the database schemas.

1. **Repo hosting and process** [enzarb]: no git history or remotes yet. Decide hosting/naming, CI, release/versioning, and image registry. Pin toolchains: `mise.toml` uses `latest`, the sandbox uses `ubuntu:26.04`, and the Rust crates need lockfiles.
2. **AuthN/AuthZ** [enzarb-ui, enzarb-sandbox-proxy]:
   - the login provider;
   - the token format the UI issues and the proxy checks (JWT, signing keys);
   - mapping connector identities to enzarb users (`app_api.identity_v`).
3. **Proxy/executor → sandbox-init trust** [enzarb-sandbox-container, enzarb-sandbox-proxy, enzarb-workflows]: mTLS or projected ServiceAccount tokens; NetworkPolicy blocking sandbox-to-sandbox traffic in the shared namespace (ADR-0013); per-Project egress policy.
4. **Secrets** [enzarb-sandbox-operator, enzarb-connectors]:
   - where the Claude OAuth token and the GitHub App private key live;
   - **whose Claude token pays for webhook-triggered runs** (the project owner?);
   - keeping tokens unreadable if sandboxes ever get Kubernetes API access (ADR-0013 limits it to other namespaces).
5. **Prompt injection** [enzarb-workflows, enzarb-connectors]: issue authors steer an agent that holds write credentials. Mitigations:
   - short-lived, single-repo clone tokens for each run;
   - only users with write access can approve;
   - the agent pushes to branches only, never to protected branches;
   - consider restricting triage on public repos to known actors.
6. **Worktree mode** [enzarb-sandbox-container]: worktree slots per sandbox, cleanup, and conflicts with someone working interactively in the same sandbox. **Fresh mode** depends on agent-sandbox warm pools/claims; confirm the upstream API in v0.5.x.
7. **Container runtime in the sandbox** [enzarb-sandbox-container]: is rootless dockerd still needed now that deploys are out of scope (e.g. test suites that use containers)? It conflicts with gVisor/Kata and with kind parity. Default: drop it. Also needed: CPU/memory quotas per sandbox and per run.
8. **Proxy availability** [enzarb-sandbox-proxy, enzarb-proto]: websockets are stateful. Running several replicas needs reconnect and resume semantics in the protocol and timeouts on flow-control credit. Workspace persistence (PVC lifecycle, hibernate/resume) is also open.
9. **pgop for a durable engine** [enzarb-kubernetes, pgop]: max 1 replica; backups, PITR and pooling aren't documented. LISTEN needs a direct session-mode connection. Confirm whether object-level grants are possible in pgop. See [database.md](database.md#known-pgop-gaps-for-this-use).
10. **Transcript storage** [enzarb-workflows, enzarb-sandbox-container]: which object store (S3-compatible? MinIO in dev?), retention, and access control for transcripts.
11. **Observability** [all]: tracing from connectors → workflows → sandbox, with the delivery id as the correlation id; metrics; workflow admin views for stuck items, retries and dead jobs; audit of terminal sessions; limits on Claude usage and CPU.
12. **CRD versioning** [enzarb-sandbox-operator]: a conversion strategy for `v1alpha1`, and status conditions designed so the UI's boot checklist maps 1:1 to them.
13. **Testing** [all]:
    - table tests for pure transitions;
    - engine tests against real Postgres (testcontainers);
    - shared enzarb-proto test vectors for Rust and TS;
    - operator envtest;
    - end-to-end on kind with recorded webhook fixtures, plus a smee-style relay for live GitHub.
14. **SvelteKit remote functions** [enzarb-ui]: they are still behind an experimental flag (`experimental.remoteFunctions`). Pin versions. Browsers connect to `sandbox-proxy` directly for websockets.
