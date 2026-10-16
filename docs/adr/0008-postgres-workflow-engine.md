# 0008. In-house workflow engine on Postgres

## Context
Workflows are long-running and involve people. A new issue can wait days for answers or approval. Workflow state must survive crashes. Storing it in custom resources would put high-churn state in etcd. The options considered were Temporal, Restate, DBOS/Hatchet, a Postgres queue with our own state machines, and NATS JetStream.

## Decision
- **Build our own** on Postgres, in `enzarb-workflows`.
- **Explicit state machines** with pure `transition(state, event) -> (state, effects)` functions. No replay-based durable execution.
- **An `events` inbox** with a unique dedupe key; everything that advances a workflow is an event.
- **The reducer** applies one event per transaction, holding a row lock on the work item, and queues the resulting jobs in the same transaction.
- **Executors** claim jobs with `SKIP LOCKED` plus a lease.
  - Agent runs are started idempotently in sandbox-init (`run_id = job.id`), and their completion comes back as an event.
- **A ticker** for timers; LISTEN/NOTIFY with polling as the safety net.

See [workflows.md](../workflows.md).

## Consequences
- **Gains:** one fewer system to run; Postgres is needed anyway; state is queryable for the UI; all Rust.
- **We own** retries, leases, timers and admin tooling, and must keep the scope small (no generic engine).
- **Postgres availability** now bounds workflow durability ([gaps.md](../gaps.md) #9).

## Amendment 2026-10-15: run results are pulled, push facts verified
- **Pulled, not called back.** The executor keeps one `executor` mux session per sandbox with accepted agent runs, subscribes to them (`run_subscribe`) and ingests `run_finished` pushes; a 15 s results pass also asks `run_status` (which now returns the full result, `run_state.result`) for every accepted run. Sandboxes advertise this as `welcome.features` `run_results`; they keep results on disk across restarts.
- **No token in the sandbox.** `run_start` to such a sandbox carries no `callback_url`/`callback_token` (now optional and deprecated), so the agent, which shares sandbox-init's uid, can no longer read a bearer token from `callback.json` and forge a result. The HTTP callback stays for older sandboxes until they roll ([gaps](../gaps.md) #20).
- **Push facts come from the remote** (#192): `pushed` is true iff the job's branch is on the remote at the reported `head_sha` (or ahead of the base when no sha was reported); a recovery run keeps the original run's push facts.
- **Rollout:** proto, connectors, sandbox-container, then workflows (any order works: an old executor ignores the feature and still sends a callback; a new executor falls back to the callback for an old sandbox).
