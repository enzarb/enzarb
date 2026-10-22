# Open questions and known gaps

The owning repo is in brackets. Resolve 2–5 first: they shape the CRDs, the protocol handshake and the database schemas.

1. **Repo hosting and process** [enzarb]: no git history or remotes yet. Decide hosting/naming, CI, release/versioning, and image registry. Pin toolchains: `mise.toml` uses `latest`, the sandbox uses `ubuntu:26.04`, and the Rust crates need lockfiles.
2. **AuthN/AuthZ** [ui, sandbox-proxy]:
   - the login provider;
   - the token format the UI issues and the proxy checks (JWT, signing keys).
3. **Proxy → sandbox-init trust** [sandbox-container, sandbox-proxy]: mTLS or projected ServiceAccount tokens; NetworkPolicy blocking sandbox-to-sandbox traffic in the shared namespace (ADR-0013); per-Project egress policy.
4. **Secrets** [operator, sandbox-proxy]:
   - where the Claude OAuth token (resolved: encrypted in Postgres, [database.md](database.md)) and the GitHub App private key live;
   - keeping tokens unreadable if sandboxes ever get Kubernetes API access (ADR-0013 limits it to other namespaces).
5. **Prompt injection** [sandbox-container]: content the agent reads (repository files, web pages) can steer an agent that holds the owner's Claude token and write-capable tooling. Mitigations today: the sandbox boundary (gVisor, egress NetworkPolicy, no Kubernetes API), chats get no push token, and only the Project owner can open a chat.
6. **Worktree mode** [sandbox-container]: worktree slots per sandbox, cleanup, and conflicts with someone working interactively in the same sandbox. **Fresh mode** depends on agent-sandbox warm pools/claims; confirm the upstream API in v0.5.x.
7. **Container runtime in the sandbox** [sandbox-container]: resolved by ADR-0012 and ADR-0016 (rootful dockerd inside the sandbox container under gVisor; rootless is not used). Still open: CPU/memory quotas per sandbox and per run. dockerd and inner containers share the sandbox container's limits, so a heavy build can OOM the agents too. Image pruning on the docker volume is tracked separately.
8. **Proxy availability** [sandbox-proxy, proto]: websockets are stateful. Running several replicas needs reconnect and resume semantics in the protocol and timeouts on flow-control credit. Workspace persistence (PVC lifecycle, hibernate/resume) is also open.
9. **pgop for durable data** [kubernetes, pgop]: max 1 replica; backups, PITR and pooling aren't documented. LISTEN needs a direct session-mode connection. Confirm whether object-level grants are possible in pgop. See [database.md](database.md#known-pgop-gaps-for-this-use).
10. (removed)
11. **Observability** [all]: sandbox utilization metrics (the `stats` frame exists; nothing polls it at the moment); audit of terminal sessions; limits on Claude usage and CPU.
12. **CRD versioning** [operator]: a conversion strategy for `v1alpha1`, and status conditions designed so the UI's boot checklist maps 1:1 to them.
13. **Testing** [all]:
    - table tests for pure logic;
    - receiver and realtime tests against real Postgres (testcontainers);
    - shared `proto/` test vectors for Rust and TS;
    - operator envtest;
    - end-to-end on kind with recorded webhook payloads, plus a smee-style relay for live GitHub.
14. **SvelteKit remote functions** [ui]: they are still behind an experimental flag (`experimental.remoteFunctions`). Pin versions. Browsers connect to `sandbox-proxy` directly for websockets.
15. (removed)
16. (removed)
17. (removed)
18. (removed)
19. **Secrets readable by the agent without a permission policy** [sandbox-container, operator]: since the ADR-0011 amendment of 2026-10-15 every agent permission request is allowed, so what the agent uid (`enzarb`, uid 1000, the same uid as sandbox-init) can read is bounded only by what is passed or mounted. Not readable: the clone/push token (never in the agent's environment, passed only to sandbox-init's own short-lived git processes as `GIT_CONFIG_*` environment). Readable: the Claude token (its own environment and the legacy `/workspace/.enzarb/claude-token`); the mux auth token at `/var/run/enzarb/auth/token` (mode 0400, owned by the pod's uid); other chats' transcripts; and, while they run, the environment of other same-uid processes through `/proc/<pid>/environ`, which includes a concurrent push's `Authorization` header (sandbox-init itself is non-dumpable; not verified under gVisor). Closing these needs a separate uid for the agent (or not mounting the auth token into the agent's reach), not filtering.
20. (removed)
21. (removed)
22. (removed)
23. (removed)
24. **Workspace data outside tagged caches is never pruned** [sandbox-container]: the workspace pruner (`workspace_prune.rs`) removes only directories with a valid `CACHEDIR.TAG` and the shared `.cache/target` dirs. Clones, step workspaces, `node_modules`, sccache and the npm cache stay, so a workspace can sit near full with nothing left to prune. When a pass ends still short, the pruner logs a `low_disk` warning with the largest `tasks/*` and `.cache/*` entries. Whether sccache/npm caches should be pruned too is open until that log shows a gap.
25. **ez `claude-code` driver limits** [ez] ([ADR-0028](adr/0028-ez-coding-agent.md)): the driver (`ez/crates/ez-claude`) is tested only against a fake claude-agent-acp (`ez/crates/ez-core/tests/fixtures/fake_claude_acp.py`), not yet end to end with a real Claude Code and token. Known limits:
    - the system prompt is fixed when the first prompt starts the child, so a mid-session mode switch changes the tools (the next `tools/list`) but not the prompt; Claude Code may also cache the tool list, and a stateless MCP server can't send `tools/list_changed`;
    - `session/load` (resume after a sandbox-init restart) starts a fresh Claude Code session without the earlier conversation;
    - Claude Code compacts its own context, so ez's compaction settings don't apply;
    - usage is per `session/prompt` (summed over Claude Code's internal requests); `model_requests` counts prompts, not model calls.
26. **ez chats are Claude-only and the model is not configurable** [sandbox-container, helm, ui] ([ADR-0028](adr/0028-ez-coding-agent.md)): sandbox-init writes a fixed ez config whose only profile is `provider = "claude-code"`, so an ez chat still needs the Project's Claude token (open fails with `NO_CREDENTIAL` without it) even though ez can talk to Anthropic and OpenAI-compatible endpoints itself. `ENZARB_CHAT_MODEL` is not a Helm value yet. Planned: per-mode backends from Project secrets (profiles via `api_key_env`), chosen in the modes designer.
27. **tracks is stateless so far** [tracks, helm, ui] ([ADR-0029](adr/0029-ez-workflows.md)): `enzarb-tracks` only parses, validates, renders and dry-runs; it has no database, routes no events and starts no runs. The dry run takes the events inline from the UI instead of reading `event_ids` from Postgres. `POST /v1/workflows/render` re-serializes the whole file, so comments in `.ez/workflows.toml` are lost on a designer save (`/v1/modes/render` keeps them). The chart ships it off (`tracks.enabled: false`) until its first image is released. The modes designer (Project → Modes) gets an empty catalog for mise tasks, MCP servers and profiles (tracks has no checkout), so those are free text and unknown names are only caught by ez at run time. The workflow designer (Project → Workflows) dry-runs drafts over logged events but its "Test run" stays disabled until tracks executes steps (ADR-0029 rollout step 4).
