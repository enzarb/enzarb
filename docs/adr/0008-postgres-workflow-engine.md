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
