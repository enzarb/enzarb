# Engine v2 contracts (ADR-0022)

This file is the shared interface for the parallel milestones:
- **M3–M5:** engine, sandbox and connectors;
- **M6:** UI;
- **M2:** library.

The interpreter types in `workflows/crates/flow` (`TaskState`, `TaskEvent`, `Effect`, `RunStep`, `Projection`, `TaskDocument`) are authoritative. This file only adds storage and the read API. Change it in the same commit as the code that needs the change.

## Postgres schema `engine` (owned by workflows; replaces `workflows`/`workflows_api`)
| Table | Columns (key ones) | Notes |
|---|---|---|
| `tasks` | `id bigserial`, `project text`, `repo text`, `binding text`, `workflow_ref text`, `state jsonb` (TaskState), `resolved jsonb` (Resolved, pinned), `origins jsonb`, `created_at`, `updated_at`, `ended_at` | `state` and `resolved` are opaque to everything but the engine |
| `subjects` | `task_id`, `kind text` (issue, pull_request, chat_thread), `key text unique` (`github:o/r#n`), `url`, `linked_at` | An object belongs to at most one live task (partial unique index where the task is not ended) |
| `relations` | `task_id`, `kind text` (parent, child, blocked_by), `target_key text`, `source text` | `blocks` is derived |
| `events` | `id bigserial`, `dedupe_key text unique`, `project`, `subject_key`, `task_id null`, `type text`, `payload jsonb` (a catalog event), `received_at`, `processed_at` | The inbox. Unrouted events are processed with no task (kept for audit) |
| `transitions` | `task_id`, `event_id`, `from jsonb`, `to jsonb`, `effects jsonb`, `at` | Append-only, used for replay |
| `jobs` | `id`, `task_id`, `kind text` (run_step, steer, stop_step, call_action, refresh_fact), `input jsonb`, `status`, `attempt`, `run_after`, `lease_owner`, `lease_until`, `output jsonb`, `idempotency_key unique`, `priority int`, `concurrency_group text null` | The generic executor queue. No per-kind logic beyond dispatch |
| `step_runs` | `run_id text pk`, `task_id`, `node`, `step`, `attempt`, `runtime`, `model`, `started_at`, `ended_at`, `active_secs`, `queue_secs`, `exit`, `tokens jsonb`, `cost_usd numeric`, `resumes int`, `stalls int`, `steers int`, `sandbox jsonb` | Metrics (ADR-0022 §10) |
| `session_events` | `run_id`, `seq int`, `at`, `kind text` (prompt, user, assistant, thinking, tool_call, tool_result, usage, steer, system, stdout, stderr), `payload jsonb`, `truncated bool`, PK `(run_id, seq)` | Partitioned by month on `at`; GIN index on `payload`; redacted before insert |
| `facts` | `task_id`, `name text`, `value jsonb`, `source text`, `at` | Verified only (git, GitHub) |
| `secrets` | `scope text` (user, project, instance), `owner text`, `name text`, `ciphertext bytea`, `updated_at` | Encrypted with a key from a Kubernetes Secret |
| `projection` | `task_id pk`, `project`, `title`, `subjects jsonb`, `status`, `end_status`, `phase`, `node`, `column`, `waiting_on`, `reason`, `text`, `actions jsonb`, `artifacts jsonb`, `needs_you bool`, `cost_usd`, `updated_at` | Written in the same transaction as each transition. **The UI's only task source.** `NOTIFY engine_projection, '<task_id>'` |

## Read and write API for the UI (schema `engine_api`, SQL, granted to the UI role)
- **Projection reads:**
  - `engine_api.task_v`: projection rows plus `project`;
  - `engine_api.task_detail(task_id)`: the projection plus `transitions` (a summary), `step_runs` and `artifacts` with content.
- **Session reads:** `engine_api.session_v(run_id, after_seq, limit)`, and a search `engine_api.session_search(project, query)`.
- **Board:** `engine_api.board_v(project)` returns the resolved board (`columns[]`) for a Project.
- **Workflow viewer:** `engine_api.workflow_v(project, repo)` returns the resolved bindings and workflows (the phase list, params, exits, origins), read-only.
- **Metrics:** `engine_api.metrics(project, from, to)`, aggregated by workflow, phase and step.
- **Write path:** `engine_api.submit(task_id, actor jsonb, kind text, data jsonb) -> event_id`, with kind one of:
  - `command` (`{command, args}`, which is what a declared action button posts);
  - `comment` (`{body}`);
  - `cancel`, `resume`, `stop`, `steer` (`{body}`).

  It inserts a catalog event with source `ui`. The engine decides everything; the UI never validates state.
- **Secrets:** `engine_api.secret_set(scope, owner, name, plaintext)` and `secret_list(scope, owner)`, which returns names only.
- **Chat:** chats are not tasks, but their sessions use `session_events` keyed by a `run_id` prefixed `chat-`. `engine_api.chat_v(user)` lists them.

## Sandbox protocol (proto, defined in M4)
- **Step frames:**
  - `step_start {run_id, attempt, workspace(s), exec: agent{runtime, prompt, model, result_schema?, outputs[], secrets{env}, github_token} | command{run, tools, outputs}}`;
  - `step_steer {run_id, text}`;
  - `step_stop {run_id}`;
  - `step_resume {run_id}`;
  - `session_event {run_id, seq, …}` (streamed);
  - `step_finished {run_id, exit, outputs{}, result?, git{branch, head_sha, pushed}}`.

  The git facts are re-verified by the engine.
- **Chat:** `chat_*` frames as today (ADR-0014), re-pointed at runtimes and session capture.

## Connectors (M5)
- **Event catalog:** each provider webhook maps to a `flow::events` catalog event.
- **Generic actions:**
  - `comment`, `label`, `unlabel`;
  - `open_pull_request`, `update_pull_request`, `merge_pull_request`;
  - `create_issues`, `close_issue`, `set_base`, `update_branch`, `delete_branch`;
  - `read_file`, `resolve_ref`.
- **Facts:**
  - `ci(sha)` with failing checks and log excerpts;
  - `reviews(pr)`, `mergeable(pr)`, `contains(sha, ref)`;
  - `dependencies(issue)`, `permission(actor, repo)`.
