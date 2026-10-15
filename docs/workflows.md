# Workflow engine (`workflows/`)

An in-house, durable workflow engine on Postgres. It doesn't use Kubernetes custom resources, because etcd can't take the throughput, and it doesn't use a third-party engine ([ADR-0008](adr/0008-postgres-workflow-engine.md)).

## Model

Each workflow is an **explicit state machine** with a **pure** transition function:

```rust
fn transition(state: &State, event: &Event) -> (State, Vec<Effect>)
```

- **Everything that advances a workflow is an event:** connector webhooks, UI actions, job results, timers.
- **Effects become jobs:** run an agent step, post a comment, open a PR, set a progress reaction. Jobs run outside the transaction and report their outcome as new events.
- **No replay and no determinism rules.** State is stored explicitly, so a workflow version change only has to read older `state_data` shapes (`workflow_version` column).
- Because transitions are pure, they get table-driven unit tests with no database.

## Tables (schema `workflows`, sqlx migrations)

| Table | Purpose | Key columns |
|---|---|---|
| `events` | Inbox: every input to the engine | `id, source, dedupe_key UNIQUE, work_item_key, type, payload jsonb, received_at, processed_at` |
| `work_items` | One row per issue / PR / mention task | `id, key UNIQUE, project_id, workflow, workflow_version, state, state_data jsonb, wake_at, updated_at` |
| `transitions` | Append-only history; feeds UI checklists and audit | `work_item_id, from_state, to_state, event_id, effects jsonb, at` |
| `jobs` | Effects waiting to run, running, or done | `id, work_item_id, project_id, kind, priority, idempotency_key UNIQUE, status, attempt, max_attempts, run_after, lease_owner, lease_until, input jsonb, output jsonb` |

Key formats:
- `work_items.key` is source-neutral: `github:org/repo#123`, `gitlab:group/proj!45`.
- `events.dedupe_key`: `github:<X-GitHub-Delivery>`, `job:<job_id>:<attempt>`, `timer:<work_item_id>:<wake_at>`, `ui:<uuid>`.
- `jobs.status`: `queued | running | succeeded | failed | cancelled`.

Concurrency caps (agent jobs per project, worktree slots per sandbox) are enforced in the job-claim query by counting `running` jobs. A separate table is added only if that gets too slow.

## Processes

All of them are roles of one `enzarb-workflows` binary and can be scaled separately.

### Ingest (called by connectors and UI)
`workflows_api.ingest_event(...)`: `INSERT … ON CONFLICT (dedupe_key) DO NOTHING`, then `NOTIFY workflow_events`. Receivers never run workflow logic inline.

### Reducer: one transaction per event
```sql
BEGIN;
SELECT … FROM events WHERE processed_at IS NULL ORDER BY id
  FOR UPDATE SKIP LOCKED LIMIT 1;
SELECT … FROM work_items WHERE key = $1 FOR UPDATE;   -- or INSERT if new
-- transition(state, event) in Rust
UPDATE work_items SET state = …, state_data = …, wake_at = …;
INSERT INTO jobs … ON CONFLICT (idempotency_key) DO NOTHING;
INSERT INTO transitions …;
UPDATE events SET processed_at = now();
NOTIFY work_item_changed, '<work_item_id>';
COMMIT;
```
- **Exactly once:** each state change is applied once.
- **Atomic effects:** the jobs it queues commit in the same transaction.
- **Ordered per issue/PR:** the row lock on the work item serializes events for that item.
- **Unrouted events:** an event whose key matches no Project is marked processed with an `ignored` transition, kept for auditing.

### Executors
- **Claiming:** jobs are claimed with `FOR UPDATE SKIP LOCKED`, ordered by `priority DESC, run_after`, and taken with a lease (`lease_owner`, `lease_until`).
- **Short jobs** (connector calls, reactions): run, write `output`, insert a `job:<id>:<attempt>` result event.
- **Agent jobs:**
  1. Resolve the sandbox. In `worktree` mode, use the Project sandbox and create worktree `run-<job_id>`. In `fresh` mode, claim a sandbox from the warm pool.
  2. Over enzarb-proto, send `StartRun { run_id: job.id, repo, ref, prompt, credentials }`. The start is **idempotent**: a repeated `run_id` returns the existing run.
  3. Release the job lease. The job stays `running` with a deadline.
  4. sandbox-init sends `RunFinished { run_id, outcome, summary, transcript_url, usage? }`, which the executor ingests as a result event. `usage` (tokens, models and the cost Claude reported) is added to `jobs.usage` once per `run_id` for every outcome, so a job's total covers all its runs; see contracts section 6.
- **Crash recovery:** when a lease expires, another executor re-sends `StartRun`, which is safe because it is idempotent.
- **Retries:** exponential backoff through `run_after` until `max_attempts`, then a `JobFailed` event.

### Ticker
Turns `work_items.wake_at` and expired job deadlines into `TimerFired` events: nudges, question timeouts, runaway runs.

### Sandbox stats poller
A single-leader loop (Postgres advisory lock) polls each ready sandbox for CPU, memory, storage and I/O counters every `ENZARB_STATS_INTERVAL` (30 s) and writes `workflows.sandbox_stats`. Unreachable or not-ready sandboxes are skipped with per-project backoff, never zero-filled. Rows are pruned after `ENZARB_STATS_RETENTION` days (90). It runs as its own task of the `ticker` role (`engine::sandbox_poller`), so a hung sandbox never delays the ticker. Each sandbox has its own timeout (`ENZARB_STATS_TIMEOUT`, 5 s), at most `ENZARB_STATS_CONCURRENCY` (8) are asked at once, and start times are jittered per project. A sandbox whose `welcome` lacks the `stats` feature, or that answers `unknown_message`, is skipped. Each sample is attributed to the lowest work item with a running `agent_*` job on the Project, or NULL. The leader prunes at most hourly. See [ADR-0019](adr/0019-sandbox-utilization-polling.md).

### Wakeups
LISTEN/NOTIFY gives low latency, and a polling sweep every few seconds acts as the safety net. LISTEN connections must be direct, session-mode connections, never through a transaction-mode pooler.

## Workflows

### `issue`
```mermaid
stateDiagram-v2
  [*] --> Triaging: WorkItemOpened
  Triaging --> AwaitingAnswers: job done, questions posted
  AwaitingAnswers --> Triaging: CommentAdded (author)
  AwaitingAnswers --> Stale: TimerFired
  Triaging --> Duplicate: job done, duplicate found
  Triaging --> Planning: job done, enough info
  Planning --> AwaitingApproval: plan comment posted
  AwaitingApproval --> Planning: CommentAdded (feedback)
  AwaitingApproval --> Implementing: ApprovalGranted (write access)
  Implementing --> ChangeRequestOpened: job done, PR opened
  ChangeRequestOpened --> Done: spawns change_request workflow
  Triaging --> Planning: job done, epic (too big for one PR)
  AwaitingApproval --> Decomposing: ApprovalGranted (epic breakdown)
  Decomposing --> AwaitingChildren: create_sub_issues done
  AwaitingChildren --> Done: all sub-issues Done
  Triaging --> Failed: JobFailed
  Implementing --> Failed: JobFailed
  Decomposing --> Failed: JobFailed
  Duplicate --> [*]
  Done --> [*]
  note right of AwaitingApproval
    Any state: WorkItemClosed → Cancelled (cancels jobs)
  end note
```

#### Epics

An issue that is too big for one pull request is broken into sub-issues instead of being implemented (docs/adr/0018-epics.md).

- **Triggers.** Triage answers `epic`; or someone with write access comments `/epic` on its own line, or adds the `epic` label (the connector reports it as `WorkItemLabeled` with `data.label = "epic"`). `/epic` and the label are accepted in `Triaging`, `AwaitingAnswers` and `Stale` (remembered: `IssueData.epic`, the plan is then a breakdown) and in `AwaitingApproval` (the plan is rewritten as a breakdown). From anyone else they do nothing: the label is rejected and `/epic` is an ordinary comment.
- **Breakdown.** Planning an epic uses `plan_epic.md`. The plan body ends its **Sub-issues** list with one fenced block whose info string is `enzarb-subissues`: a JSON array of `{key, title, body, depends_on: [key]}`. The engine validates it (`parse_epic_plan`): at most `Config::max_epic_children` (default 8) entries, unique `[a-z0-9_-]{1,32}` keys, non-empty titles, `depends_on` naming other entries, no cycle. An invalid list fails the plan step (retry from the UI) and nothing is created. The list is stored in creation order (`IssueData.sub_issues`).
- **Approval.** `/approve` (or the UI button) moves the epic to `Decomposing` and queues one `create_sub_issues` job `{item, depth, sub_issues}`. It never starts `Implementing` and the epic never opens a change request.
- **Creation.** For each entry, in order, the connector creates an issue whose body is the plan's text, `Part of the epic #N.`, `Blocked by #X` for each dependency (parsed as text dependencies like any other), and the hidden markers `<!-- enzarb:epic=<epic key>:<key> -->` and `<!-- enzarb:epic-depth=<n> -->`. The idempotency key is `epic-<epic key>-<key>`; the GitHub connector looks for an existing issue carrying it before creating, so a retried job or replayed event makes no duplicates. Then each is linked as a sub-issue (`Connector::link_sub_issue`: GitHub's sub-issues API, else a task list in a managed section of the parent's body), native "blocked by" relationships are added best-effort, and the job records the structure in `work_item_dependencies` with source `epic` (the epic waits for each sub-issue; a sub-issue waits for what it is ordered after). A failure part-way fails the job; **retry** (the `Failed` item's retry action) runs it again and resumes through the markers.
- **Sub-issues are normal issues.** enzarb opens them, so the receiver lets that one bot event through and the engine accepts it only if the marker names a parent that is in the same repository and Project, is `Decomposing` or `AwaitingChildren`, whose approved plan lists that key, and the actor is enzarb's own login. The depth comes from the stored parent (`epic_depth + 1`), not from the body. Each sub-issue is then triaged, planned and approved like any other issue: nothing is implemented without a human `/approve`. `Config::max_epic_depth` (default 1) is the depth at which an issue can no longer be an epic: triage's `epic` becomes `ready` and `/epic` and the label are rejected there.
- **Waiting.** In `AwaitingChildren` the epic is blocked by its sub-issues. The ticker's dependency sweep sends `DependenciesChanged` when the open set changes; the machine stores it (`IssueData.open_dependencies`). When none is left the epic is `Done` with a summary comment. If every remaining sub-issue was closed without completing, the epic comments once and waits; a maintainer can reopen or replace them, or `/approve` to close the epic as it is. Cancelling or closing the epic cancels only the epic: sub-issues stay open. "Done" for a sub-issue is the same as for any issue (its change request was opened), not merged.

### `change_request`
Starts for every opened PR/MR, whether the agent opened it or not.
```mermaid
stateDiagram-v2
  [*] --> Reviewing: ChangeRequestOpened
  Reviewing --> AwaitingChanges: review posted
  AwaitingChanges --> AddressingComments: ReviewSubmitted / CommentAdded
  AddressingComments --> AwaitingChanges: job done, commits pushed
  AwaitingChanges --> Reviewing: new commits (synchronize)
  AwaitingChanges --> ReadyToMerge: CI ok, review ok, mergeable ok
  ReadyToMerge --> Merging: UI merge action (write access, readiness all ok)
  ReadyToMerge --> Reviewing: new commits
  ReadyToMerge --> AwaitingChanges: CI red or review changes
  Merging --> Merged: merge job result merged / already_merged
  Merging --> Reviewing: head_moved
  Merging --> AwaitingChanges: not_mergeable (merge_error set)
  Merging --> Reviewing: updated_branch (behind base: base merged into the branch, CI reruns)
  AwaitingChanges --> ResolvingConflicts: MergeConflictDetected (base_sha)
  ReadyToMerge --> ResolvingConflicts: MergeConflictDetected (base_sha)
  ResolvingConflicts --> Reviewing: conflicts resolved, merge commit pushed
  ResolvingConflicts --> ResolvingConflicts: blocked, 1st attempt on this base_sha
  ResolvingConflicts --> ConflictParked: blocked, 2nd attempt on this base_sha
  ConflictParked --> ResolvingConflicts: retry / mention (write access) or new base_sha
  AwaitingChanges --> Merged: WorkItemClosed (merged)
  AwaitingChanges --> Closed: WorkItemClosed
  Merged --> [*]
  Closed --> [*]
```

#### Automatic fix rounds and queued feedback
- When enzarb's own review finds must-fix problems (`request_changes`, or `comment` only when an unverified `approve` was downgraded; a plain `comment` waits for a person), the item goes `Reviewing → AddressingComments` by itself with the review body as the trigger, and the fix is re-reviewed. At most `ENZARB_MAX_AUTO_ROUNDS` (default 3, `0` = off, Helm `workflows.maxAutoRounds`) such rounds run in a row; a human comment or review, or a new head pushed by someone else, resets the count. After the cap the item stays in `AwaitingChanges` with `state_data.wait.reason = round_limit`. An `approve` goes to `ReadyToMerge` as before.
- A CI failure on the reviewed head of an `AwaitingChanges` (or `ReadyToMerge`) item starts one `AddressingComments` round per head SHA (`ci_fix_sha`), whose trigger lists the failing checks with their URLs. It counts against the same `auto_rounds` cap. A failure during `Reviewing`, `AddressingComments` or `ResolvingConflicts` is recorded and acted on when that run ends. With the cap reached the item waits on a human with `state_data.wait.reason = ci_failed`; Retry resets the count and starts a round.
- A human comment or review that arrives during `Reviewing` or `Merging` is queued like one during `AddressingComments` (`state_data.pending`). When the review posts, or the merge ends in `head_moved` or `not_mergeable`, the queued text starts the next `AddressingComments` round before any automatic one; if the merge succeeds it is dropped.
- Why the item waits is only `state_data.wait` (contracts §6): `enzarb_fixing` while an automatic round runs, `round_limit` at the cap, `awaiting_review` when a person has to review.

#### Merging
enzarb never merges its own PRs on its own. A person triggers the merge from the UI.

- **Entering `ReadyToMerge`:** from `AwaitingChanges` once all three readiness checks in `state_data.checklist` (`ci`, `review`, `mergeable`) are `ok`:
  - `ci`: every check run and commit status on the head commit passes, not only branch-protection required checks. A repo with no CI counts as passing. The aggregation rule is in [connectors.md](connectors.md); `CiStatusChanged` events (from `check_suite`, `check_run` and `status` webhooks) trigger a re-evaluation.
  - `review`: enzarb's own review of the current head approves. No human GitHub review is required (see [gaps.md](gaps.md)); the Merge click is the human gate.
  - `mergeable`: the provider reports the PR as mergeable (`get_merge_readiness`).
- **Leaving `ReadyToMerge`:** new commits send it back to `Reviewing`; CI going red or review changes send it back to `AwaitingChanges`; `WorkItemClosed` closes it as usual.
- **Merge action:** `submit_action(..., 'merge')` produces `MergeRequested`, accepted only in `ReadyToMerge`. The reducer rejects it (a `transitions.note`, not a SQL error) if the clicking user lacks write access (`Connector::check_permission` on the actor login) or if readiness is no longer all `ok`. On acceptance the state is `Merging` and a merge job runs `merge_change_request`, pinned to the head SHA it was approved at.
- **Merge job results:**
  - `merged` or `already_merged` → `Merged`;
  - `head_moved` → `Reviewing`;
  - `not_mergeable` → `AwaitingChanges` with `state_data.merge_error` set.
- **Branch deletion:** `merge` may carry `data.delete_branch` (the review dialog's "Delete branch after merge"). It is kept in `state_data.merge_delete_branch` as long as `merge_requested`, and the merge job's input then names the head branch. After `merged` or `already_merged` the job deletes it with `Connector::delete_branch`, best effort, and returns what happened as `output.branch`; the `Merged` transition's note repeats it. The branch is **kept** (with a note) when it is not an `enzarb/...` branch, or when an open change request has `stacked_on` = this one: GitHub would change that child's base by itself without rebasing, which defeats the retarget below. The user can delete the branch later. A delete that fails (protected branch, missing permission) is only a note.
- **Comments from the UI:** `submit_action(..., 'comment')` produces a `CommentAdded` event with source `ui` and `data.body`. On an issue it takes the same path as a GitHub comment (`AwaitingApproval`: plan feedback re-plans, unless it is an `/approve` command; `AwaitingAnswers`: only the author answers) and also queues a `post_comment` job with the text (signed "Sent by @login from enzarb"). The bot's own comment coming back through the webhook is ignored, so one UI comment re-plans once. Change requests and mentions reject it.
- **Behind the base:** when the readiness re-check inside the merge job reports `mergeable_state == "behind"`, it does not merge. It calls `Connector::update_branch` (GitHub `PUT /pulls/{n}/update-branch`: a merge of the base into the branch, never a rebase or force-push) and returns `updated_branch`. The new head is reviewed and CI reruns; the user's click is stored (`state_data.merge_requested`, with `merge_actor`), so once the new head is approved, CI is green and readiness is ok the merge runs without a second click (wait reason `updating_branch` meanwhile). Requested changes, a failed check or a refused merge clear the stored request. A refused update (conflict) is `not_mergeable`.
- **Serialised per repository:** the merge job holds `pg_try_advisory_xact_lock(hashtext('merge:<repo>'))` for the duration of the readiness re-check and merge. If another merge of the same repository holds it, the job is re-queued untouched (a transient error with a short `retry_after`). The lock ends with the transaction, so a crashed worker cannot leave it held. Merges in different repositories do not block each other. A PR left behind by an earlier merge is then updated (above) or sent to conflict resolution before it merges.
- **Conflict resolution:** `MergeConflictDetected {base_sha}` (from the base-head poll or a readiness check that says `mergeable == false`) starts `AgentResolveConflicts`: merge the base into the branch, never rebase or force-push, run the checks, push. At most 2 attempts per `base_sha` (`state_data.resolve_attempts {base_sha, count}`; the older single `resolve_attempted_base_sha` still reads as 1 attempt). After the 2nd blocked attempt the item is `ConflictParked` and appears under Needs you with "Conflict could not be resolved automatically". A new base commit resets the count. `mergeable == null` (GitHub computes it lazily) is neither a conflict nor a final answer: the poll retries it on the next pass.
- **New commits during `Merging`** do not cancel the merge. It is pinned to the head, so GitHub refuses it and the job returns `head_moved`.
- **Squash only.** The commit title comes from `squash_title(title, head_ref)` (workflows `machine/title.rs`): the PR title when it is already a Conventional Commit, otherwise a type prefix derived for the PR. See the engine code for the exact derivation.
- **Invariant:** nothing but a UI Merge action merges. No event from a connector, comment or mention, and no timer, can cause `MergeRequested`. The only merge not started by a click is the resumption of a click already stored in `merge_requested` (see Behind the base).
- **Waiting for the merge lock** re-queues the job after 5s and refunds the attempt (`max_attempts + 1`), so contention never fails a merge.
- **Stacked PRs** (#107, on the dependency model of [ADR-0017](adr/0017-work-item-dependencies.md)):
  - *Choosing the base.* When an issue's implementation starts, the reducer looks at its dependencies (`Ctx::stack_parent`, `dependencies::stack_parent`, the pure `machine::stack::select_stack_parent`). It stacks only when exactly one dependency is not completed, that dependency has an open change request, and the request's branch is enzarb's own (`enzarb/…`). Several open parents, a parent without a change request, and a merged, closed or failed parent stack nothing: the branch starts from the default branch and the `dependencies` merge gate sequences it, as before. The agent run starts from the parent's branch, and the change request opens with the parent's branch as base. Its body ends with a hidden `<!-- enzarb:stack parent=… ref=… tip=… -->` marker; the change request machine records it as additive `state_data` keys (`stacked_on` = the parent change request's key, `stack_parent_ref`, `stack_parent_tip` = the parent head the child was cut from, `default_base`) only when enzarb authored the change request and its base is the marker's branch.
  - *While stacked*, the `dependencies` check is pending ("stacked on …: waits for it to merge"), so the child cannot reach `ReadyToMerge` and nothing can merge it into the parent's branch. If the parent is closed or cancelled the check fails and the child stays blocked under Needs you; nothing is rebased. A parent that moves after the child started is left alone.
  - *Retarget.* When the parent change request reaches `Merged`, `Closed` or `Cancelled`, the reducer sends `DependenciesChanged` to each open child with `stacked_on` = the parent (so it does not wait for the sweep). A child whose parent merged queues one `retarget_change_request` job (`{item, number, head_ref, expected_sha, old_base, onto}`; not an agent job; at most 2 per change request, `retarget_attempts`). The job re-reads the change request under the repository's merge lock (a head that is not `expected_sha` is `lease_failed`), mints a `read_write` credential and starts a sandbox **retarget run** (`RunStart.retarget`, [contracts](contracts.md)): `git rebase --onto origin/<default> <stack_parent_tip>` of the branch and a push with `--force-with-lease=refs/heads/<branch>:<expected_sha>`. When the run reports `retargeted`, the callback calls `Connector::set_base` (an already-correct base is success). Whether the rebase is still needed is decided by git (the remote head and `old_base`), not by the change request's current base: GitHub moves the base by itself when the parent branch is deleted, without rebasing.
  - *Outcomes* (`output.outcome`): `retargeted` → `base_ref` is the default branch, the stack keys are cleared, and the new head is reviewed (`Reviewing`) before the usual checks; `lease_failed` → `Reviewing` with the stack kept (the retarget runs again after the review, up to the cap, then it parks); `conflict`, `foreign_commits` (a commit on the branch not authored by enzarb), `not_ancestor` (the recorded parent tip is not in the branch's history: the tip is stale), `rejected` (not an enzarb branch, or a sandbox that does not know the operation), a failed job and a missed deadline → `ConflictParked` with a comment, stack keys cleared, so the item is under Needs you and the engine does not try again by itself. Nothing is pushed in any of these. A `retargeted` result whose base change the provider refused (`base_set: false`) is parked the same way. Retargeting never merges: the user's Merge click stays the only merge trigger.
  - *Force-push invariant.* The sandbox retarget run is the **only** place in enzarb that rewrites history or pushes with a lease. It force-pushes only to a branch enzarb created (`enzarb/` prefix), never the default or a protected branch, only at an `expected_sha`, only after checking that every commit it replays was authored by enzarb, and never as a general git passthrough. No other component names a force flag (tests scan the engine and the sandbox sources). Everything else, `update_branch` and merge-in conflict resolution included, keeps the rule of never rebasing or force-pushing.
  - *Not covered:* stacking on a parent without a change request, rebasing when the parent changes before it merges, and several parents.

### `mention`
`Mentioned` (`@enzarb …` anywhere) → one `agent_task` job → reply posted → Done. If the mention is on an item that already has a workflow, it goes to that workflow as a comment instead. Exception: an explicit `@enzarb` mention by a user with write access on a `Failed` (or, for issues, `Stale`) `issue` or `change_request` item retries it, exactly like the UI retry. Mentions on finished items (`Done`, `Duplicate`, `Cancelled`) or from users without write access are still ignored, silently.

### Graph workflows
[ADR-0020](adr/0020-graph-workflows.md), first slice. A repository opts in by committing `.enzarb/workflows/issue.yaml` to its default branch; an example is [`workflows/examples/issue.yaml`](workflows/examples/issue.yaml) (prepare, implement, open a change request). Without the file nothing changes: issues run the compiled `issue` machine.

- **Definitions.** The schema, validation (start and edge targets exist, every node can reach an `end`, no unknown types, actions or events, a merge only after a `wait` on exactly `merge_requested`), the condition language (identifiers over the last node's output, `==`, `!=`, literals, `&&`, `||`, `!`, parentheses) and `advance` (outputs, visit counts, `max_visits` default 5, `on_limit`, `optional`) live in the pure crate `workflows/crates/graph`, shared with sandbox-init. A conditional `next` must end with `else`. A failed `command` or `agent` node (`ok: false`, or a non-zero `exit_code`) fails the graph whatever its edges, unless the node is `optional`; then its edges decide (for example `{"exit_code == 0": open_pr, else: fix}` on an optional check).
- **Start.** For an issue-opening event in a Project with issue triggers, the reducer (before the transaction, like the permission lookup) reads `.enzarb/workflows/issue.yaml` with `Connector::branch_head` and `Connector::get_file` at the default branch head. When it loads and lists `issue_opened` in `on`, the item is created as `graph:issue` with the definition and its sha pinned in `state_data.graph`. A definition that does not validate, or that uses what this kernel cannot run yet (actions other than `post_comment` and `open_change_request`, waits on events other than `comment` and `approval`), falls back to the compiled machine and posts a comment naming the problem. A definition for other triggers falls back silently. Any read error, or no answer within 5 seconds, is logged and falls back too, so a provider outage never holds up events. "No definition at this head sha" is remembered for 5 minutes per repository.
- **Sandbox nodes.** Consecutive `command` and `agent` nodes run as one `agent_segment` job: a sandbox segment run (`RunStart.graph`) on `enzarb/issue-<n>` from the default branch, with push. Named agent prompts (`triage`, `plan`, `implement`, `review`, `address_comments`, `resolve_conflicts`, `task`) are the built-in templates; any other `prompt` is inline text with the same `{placeholders}`. The executor renders them with the thread, like other agent jobs. After the walk stops at a kernel node, edits left by `command` nodes are committed and pushed too. An agent node's `pushed` output is true only when HEAD moved during that node and the branch was pushed, so `next: {pushed: ...}` cannot open a change request without new work. The result (`RunFinished.graph`) is untrusted: the kernel accepts it only if `stopped_at` is a kernel node reachable from the segment's start through sandbox nodes (edges and `on_limit`), else the task fails. It takes outputs of sandbox nodes only, never lowers a visit count, keeps its own counts and outputs for kernel nodes, and counts the visit of the stop itself (with `max_visits`/`on_limit`).
- **Kernel nodes.** `post_comment` posts `body`, or the latest agent message's `body`; `open_change_request` opens `enzarb/issue-<n>` against the default branch, titled with the agent's `title` (else the issue title), the latest agent `body` plus `Closes #<n>`, and hands the change request to the `change_request` workflow as the compiled machine does. The node's output is the job's result (`{number, url}` for a change request) and the task continues on it. `wait` sets `Waiting` until a `comment` (from a user with write access; `from: anyone` on the node accepts any human; output `{event, author, body, can_write}`) or an `approval` (write access required, as today; output `{event, author}`) it lists. `end` finishes the task `Done`, `Failed` (with a comment) or `Cancelled`.
- **States.** `Running` (a job is in flight; `state_data.waiting_on` names its kind), `Waiting`, `Done`, `Failed`, `Cancelled`. Closing the issue or a write-access cancel cancels. A retry (UI retry, or an @mention by a user with write access) of a `Failed` task restarts it at `start` with the definition re-read from the default branch, or the pinned one when that lookup fails; the note says `retry; restarted at <node>`.
- **Adopting compiled `issue` items.** When the default branch has a usable issue graph (same lookup, timeout, cache and fallback as on open), an open compiled `issue` item moves to `graph:issue` (`work_items.workflow` changes, `workflow_version` stays 1) when a human acts on it:
  - an accepted approval of its plan in `AwaitingApproval` (`/approve`, the label or the UI, write access as before) places it at the definition's first `wait` on `approval` (breadth-first from `start`) and resumes it with `{event: "approval", author}` in the same transaction, so the next segment is queued at once; the compiled plan becomes `{plan}` for the graph's prompts. A definition with no such wait keeps the item on the compiled machine;
  - an accepted retry of a `Failed` or `Stale` item (UI retry, or a write-access @mention; the author answering a `Stale` issue is still an answer) starts the graph fresh at `start`.
  
  The note is `plan approved; moved to graph workflow at <node>` or `retry; moved to graph workflow at <node>`. Plan feedback, epics, items with a queued or running agent job, other states, rejected events and any lookup failure or invalid definition stay on the compiled machine, unchanged.
- **Not in this slice:** the `change_request` graph, `merge_change_request`, waits on `ci`, `review`, `merge_requested`, `dependencies_done` and `timer`, UI support for `graph:*` items beyond the generic fields, and built-in default graphs.

## Progress feedback
Whenever a transition **accepts** an event, the reducer queues a high-priority `set_progress` job:
- Received → 👀
- Working → 🚀
- Done → 👍
- Failed → 😕

These jobs skip agent concurrency caps, so the acknowledgement lands within seconds. See [connectors.md](connectors.md#progress).

## Authorization of triggers
Before a transition accepts an event, it checks:
- the actor maps to an enzarb identity (`app_api.identity_v`) or has write access (`Connector::check_permission`), as the trigger requires;
- `ApprovalGranted` always needs write access;
- the bot's own comments and reactions are ignored, which prevents loops.

## Out of the engine
Run logs and transcripts never go through these tables. Live output streams from sandbox-init to the UI through the proxy (an enzarb-proto channel). The final transcript goes to object storage, and `jobs.output` keeps only a URL.
