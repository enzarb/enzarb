`engine.sql` is a local stand-in for the engine v2 `engine` / `engine_api` schemas
(docs/workflows/contracts-v2.md), applied by `engine-fixture.ts`. TODO(M7): apply the real
`workflows/migrations` instead and delete it.

Shapes the UI assumes beyond contracts-v2 (to settle with the engine side):
- `task_v`: projection columns plus `repo`, `workflow_ref`, `created_at`, `ended_at`; `artifacts` without `content`.
- `task_detail(id) -> jsonb`: the `task_v` row plus `artifacts` (with content), `transitions`
  (`[{at, event_type, from:{status,node}, to:{status,node}, effects}]`) and `step_runs` (table columns).
- `session_v(run_id, after_seq, limit)` returns `(seq, at, kind, payload, truncated)`;
  `session_search` returns `(run_id, task_id, seq, at, kind, snippet)`.
- `board_v -> jsonb {columns:[{id,title,limit}]}`; `workflow_v(project, 'owner/repo') -> jsonb
  {bindings, workflows:[{ref, origin, phases:[{id, use, origin, params, with, exits, graph:{nodes,edges}}]}], lint}` or null.
- `metrics` returns `(workflow, phase, step, runs, avg_wall_secs, active_secs, queue_secs,
  input_tokens, output_tokens, cost_usd, retries, resumes, stalls, steers)`.
- `secret_list` returns `(name, updated_at)`; new `secret_delete(scope, owner, name) -> boolean`.
- `chat_v(user)` returns `(run_id, project, title, started_at, last_at, events)`.
- `NOTIFY engine_projection, '<task_id>'` on every projection write.
