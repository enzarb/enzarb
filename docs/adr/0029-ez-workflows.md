# 0029. ez workflows: graphs in `.ez/workflows.toml`, the `tracks` service, replay and test runs

Status: Accepted (2026-10-22). The redesign announced by [ADR-0027](0027-workflow-clean-slate.md); builds on [ADR-0028](0028-ez-coding-agent.md) (ez, modes) and [ADR-0011](0011-acp-agent-runs.md) (agent runs as step primitives).

## Context
ADR-0027 deleted the workflow engine because it had grown too complicated to reason about. The deleted ADRs (0020 graph workflows, 0022 the generic interpreter) left lessons:
- **Copies drift.** The same rule was defined in Rust, SQL and TypeScript, and the copies disagreed (0022's #215–#228).
- **Layers multiplied.** Imports, a shared library repo, phases inside graphs, board columns, subjects and relations each made sense alone, but together nobody could predict a run.
- **What was right:** behaviour as data in the repository, trusted only from the default branch; a small engine outside the sandbox that owns durable state, waits and privileged GitHub actions; the sandbox as a generic executor.

ADR-0028 adds the piece that was missing then: an agent whose prompt, tools and permissions are one configuration, the **mode**. A workflow step only has to say *which mode* and *what instruction*.

The owner wants to design modes and workflows in the UI, store them in the repository by committing directly to the default branch, and test workflows against events that were already logged, first as a dry run and then as a real run that changes nothing outside the sandbox.

## Decision

### 1. Files
Everything lives in the repository, next to the modes:

| File | Holds |
|---|---|
| `.ez/modes.toml` | modes (ADR-0028, [`ez/docs/modes.md`](../../ez/docs/modes.md)) |
| `.ez/workflows.toml` | workflows (this ADR) |

One file per kind, no imports, no shared library, no includes. A repository without `.ez/workflows.toml` has no workflows; nothing is built in.

### 2. A workflow is a trigger plus a graph

```toml
# .ez/workflows.toml
version = 1

[workflows.fix-issue]
description = "Fix issues labelled ez and open a pull request"
on = ["issues.labeled"]                        # event[.action] from connectors.event_v
if = "event.label.name == 'ez'"                # optional filter expression
start = "triage"

[workflows.fix-issue.nodes.triage]
type = "agent"
mode = "triage"                                # a mode in .ez/modes.toml (same commit)
instruction = """
Issue #{{ event.issue.number }}: {{ event.issue.title }}

{{ event.issue.body }}"""
result = { decision = { enum = ["fix", "needs_info"] }, summary = { type = "string" } }
next = [
  { if = "result.decision == 'fix'", to = "implement" },
  { to = "ask" },
]

[workflows.fix-issue.nodes.ask]
type = "action"
action = "comment"
body = "{{ nodes.triage.result.summary }}"
next = "wait_reply"

[workflows.fix-issue.nodes.wait_reply]
type = "wait"
on = ["issue_comment.created"]                 # events about the run's subject only
if = "event.comment.user.login == subject.author"
timeout = "7d"
next = [{ if = "timed_out", to = "end_stale" }, { to = "triage" }]

[workflows.fix-issue.nodes.implement]
type = "agent"
mode = "implement"
instruction = "Fix issue #{{ event.issue.number }}. Plan: {{ nodes.triage.result.summary }}"
push = true                                    # push the run branch when the node succeeds
next = [{ if = "git.pushed", to = "open_pr" }, { to = "end_failed" }]

[workflows.fix-issue.nodes.open_pr]
type = "action"
action = "open_pull_request"
title = "Fix #{{ event.issue.number }}: {{ event.issue.title }}"
body = "Closes #{{ event.issue.number }}"
next = "end_done"

[workflows.fix-issue.nodes.end_done]
type = "end"
status = "done"

[workflows.fix-issue.nodes.end_failed]
type = "end"
status = "failed"

[workflows.fix-issue.nodes.end_stale]
type = "end"
status = "cancelled"
```

**Node types** (the complete set; anything else needs a new ADR):

| Type | Runs in | Does | Outputs |
|---|---|---|---|
| `agent` | sandbox | `ez acp` in `mode` with `instruction` as the first user turn; with `result`, ez offers `submit_result` with that schema; with `push = true`, sandbox-init commits and pushes the run branch | `result`, `git {branch, head_sha, pushed}`, `exit` |
| `command` | sandbox | Either `task` + `args` (`mise run <task> -- <args>`, preferred: the task is reviewed in the repo and needs no quoting) or `run` (a shell string, `sh -c`, rendered as a template with every value shell-quoted), in the run's worktree | `exit_code`, `output` (tail), `json` (parsed stdout when it is JSON) |
| `action` | tracks | One GitHub call with the App's installation token: `comment`, `open_pull_request`, `add_labels`, `remove_labels` | the created object's `number` / `url` |
| `wait` | tracks | Suspends until an event about the run's subject matches `on` and `if`, or `timeout` passes | `event`, `timed_out` |
| `end` | tracks | Ends the run with `status` (`done`, `failed`, `cancelled`) | |

**Edges.** `next` is a node id, or a list of `{if?, to}` checked in order; the last entry has no `if`. A run whose edges all fail ends `failed` with that reason.

**Expressions and templates** are [MiniJinja](https://github.com/mitsuhiko/minijinja), one language for both: `instruction`, `body` and `title` are templates; `if` is an expression. The context is the same everywhere:
- `event`: the triggering delivery's payload (or, after a `wait`, the event that resumed it), plus `event.name` and `event.action`;
- `subject`: `{kind: issue|pull_request, number, author, url}`, derived from the trigger event;
- `nodes.<id>`: the latest outputs of every node that ran; inside `next`, the node that just ran is also bare (`result`, `git`, `exit_code`, `timed_out`);
- `run`: `{id, workflow, attempt, test}`.

An undefined variable is an error (strict undefined), not an empty string.

**Loops** are edges back to an earlier node. Every node has `max_visits` (default 5); past it the run ends `failed` (`max_visits` reason). A run also has `timeout` (default `14d`) and the workflow `max_runs` concurrent runs (default 3).

**Subjects and routing.** A run belongs to one subject (`repo#number`). An event about a subject with a live run of the same workflow is offered to that run's current `wait` (and ignored if it doesn't match); it never starts a second run of that workflow for the subject. An event matching no live run is checked against every workflow's `on`/`if`.

### 3. Validation
One pure Rust crate, **`ez-workflow`** (in `ez/crates/`), owns the schema, validation, trigger matching, template rendering, condition evaluation and `advance(state, outcome) -> state`. It is the only implementation: tracks, the dry run and the UI's validation all go through it (the UI by calling tracks, §6; no TypeScript copy).

A definition is rejected unless:
- `start` and every edge target exist, and every node can reach an `end`;
- every `agent` node's `mode` exists in `.ez/modes.toml` **of the same commit** (a mode that lists `AskUserQuestion` is only a warning: unattended runs never offer `AskUserQuestion` or `SwitchMode`);
- every template and expression parses, and references only the context names above;
- `on` names are `event` or `event.action` strings (unknown names are warnings: GitHub adds events);
- `action` nodes use known actions with their required fields;
- a `command` node has exactly one of `task` and `run`.

### 4. Trust and storage
- **Default branch only.** Tracks reads `.ez/workflows.toml` and `.ez/modes.toml` (and the instruction files ez folds into the prompt) at the default branch's head when a run starts, through the GitHub App, and pins the commit sha for the run's life. Every agent node gets the pinned modes inline (§5); the copy in the worktree is never used. A branch under work cannot change its own workflow or modes.
- **The designers commit directly.** The UI's modes and workflow designers save with the GitHub contents API (`PUT /repos/{o}/{r}/contents/{path}`), on the default branch, using the App's installation token:
  - only for a user with write, maintain or admin permission on the repository (the live check the UI already uses, `collaborators/{login}/permission`), checked at save time;
  - the request carries the blob sha the editor loaded, so a concurrent change fails with a conflict and the editor reloads instead of overwriting;
  - commit message `chore(ez): update <file> from enzarb` with a `Co-authored-by` trailer for the user, committer the App;
  - the content is validated (§3) before the commit; an invalid file is never committed;
  - branch protection that refuses App pushes is shown as the error; there is no fallback to a pull request in this ADR.
- Committing directly is the same trust as merging a PR: only people who could merge can save. Runs started after the commit use it; running runs keep their pinned sha.

### 5. Tracks
A new top-level component `tracks/` (Rust, one Deployment, its own Postgres schema `tracks`, migrations in the chart's order after `connectors`). It is the only thing that acts on events. A workflow is live as soon as `.ez/workflows.toml` on the default branch defines it: there is no per-Project switch, so test before saving (the designer's dry run and test runs work on unsaved drafts).

- **Input.** `LISTEN connector_event`; for each id it reads `connectors.event_v`, finds the Project owning `repo` (Project CRs, read-only), routes the event (§2) and records the decision (`tracks.event_decisions`: matched workflow or "no match" with the reason), so the UI can show why an event did or did not start a run.
- **State.** `tracks.runs` (workflow, subject, pinned sha, status, test flag, current node, visit counts) and `tracks.node_runs` (one row per node visit: inputs rendered, outputs, timings, usage). All transitions go through `ez-workflow::advance` in one transaction per step. Runs survive tracks restarts; waits last days.
- **Sandbox nodes** use the existing step primitives (`step_start` … `step_finished`, [protocol.md](../../proto/docs/protocol.md)). Tracks dials the Project's sandbox-init mux endpoint with the sandbox auth token, as sandbox-proxy does (RBAC: `get` Projects and the `sb-*-auth` Secrets in the sandbox namespace; the sandbox NetworkPolicy admits tracks pods again). Additive proto changes, gated by a new `welcome.features` entry `ez_steps`:
  - `StepExec.agent.mode` (a mode id) and `StepExec.agent.ez` (`{modes_toml, instruction_files: {path: content}}`, the pinned copies);
  - sandbox-init runs `ez acp` for such steps with a per-run `EZ_CONFIG_DIR` and `EZ_SESSIONS_DIR` (as for chats), starts the session in `mode`, and refuses an unknown mode;
  - the run's worktree branch is `ez/<workflow>/<run-id>`, created from the default branch; one worktree per run, shared by its nodes.
- **Credentials.** Claude-code profile runs use the Project owner's Claude token (test runs: the token of the user who started the test), delivered like chats; a run whose token is missing ends `failed` (`no_credential`); native profiles use Project secrets (ADR-0028 §5). The App installation token for `action` nodes never enters the sandbox; the push token is a short-lived installation token scoped to the repository with `contents: write`.
- **Not in tracks:** prompts, per-workflow logic, board columns. A request for workflow-specific code in tracks is a request for a new node type or action, which needs an ADR amendment.

### 6. Testing with past events
Both modes run against a definition that may be **unsaved** (the designer's draft), with the draft modes, and against events from `connectors.event_v` of the Project's repositories.

**Dry run** (no sandbox, no GitHub writes, no model calls). `POST /v1/dry-run {project, workflows_toml, modes_toml, event_ids, assume?}` on tracks (cluster-internal, called by the UI after its own access check). For each event it returns, from `ez-workflow` alone:
- validation errors and warnings;
- which workflow's trigger matches, or why none does (each `on` / `if` evaluated);
- the path through the graph: for each node, the rendered `instruction` with its mode (and that mode's tools), the rendered action body, the wait's `on`/`if`; at each branch, the edge taken;
- where the path needs an outcome it can't know (an agent's `result`, a command's exit, a wait's event), it stops and lists the edges, unless the request's `assume` gives that node's outputs. The designer offers those as choices, so the user can walk every branch.

**Test run** (real, contained). The UI asks tracks to start a run from a past event with `test = true`:
- runs in the Project's sandbox like a normal run, from the draft or the saved definition;
- branch `ez-test/<run-id>`, **never pushed**: `push` is forced off in every `step_start` and no push token is sent;
- `action` nodes are not executed: the rendered call is recorded as the node's output with a synthetic `number`/`url`;
- `wait` nodes ask the user in the UI: pick a later logged event to resume with, or "time out";
- shown in the same run view as real runs, marked Test; the worktree is removed when the run ends (kept 24 h for inspection, then pruned).

Only users with write permission on the repository can dry run or test run.

### 7. UI
- **Modes designer** (Project → Modes): a form per mode over ez's mode keys (description, prompt, tools, mise tasks from the repo's real tasks, MCP tool globs, hooks, profile/model, next/auto-advance), a raw TOML tab, validation on every edit through `ez-workflow`/ez's mode validation, Save = direct commit (§4).
- **Workflow designer** (Project → Workflows): the graph drawn with automatic layout (positions are not stored in the file), a side panel per node, the raw TOML tab, the dry-run panel over logged events, and "Test run".
- **Runs** (Project → Runs): runs and test runs, node by node, with rendered inputs, outputs, usage and the agent session events; Stop.
- **Events tab:** each event shows the decision tracks recorded (which workflow started, or why none).

### 8. Rollout
1. This ADR (review), then `ez-workflow` with the schema, validation, rendering and `advance`, fully unit tested.
2. Modes designer and direct-commit save. The UI validates through tracks, so a stateless tracks comes first: no database, no Kubernetes access, endpoints to parse, validate and render `.ez/modes.toml` and `.ez/workflows.toml`, and the dry run with the events passed inline (`events: [{id, event, action, payload}]`, which the UI reads from `connectors.event_v` after its access check) instead of `event_ids`.
3. tracks schema, routing and decisions (the dry run then also accepts `event_ids`); workflow designer with dry run. **Done** (schema `tracks`, chart migration Job, routing loop and `GET /v1/decisions`, the Events tab shows each decision; routing creates `pending` runs but nothing executes them, and the per-workflow `max_runs` limit is not enforced yet).
4. ez step execution in sandbox-init (the additive proto fields, ADR-0028 rollout steps 3 and 5); tracks runs `agent`, `command` and `end` nodes; test runs.
5. `action` and `wait` nodes; real runs start for every saved workflow.
6. Per-mode backends with Project secrets (the native driver).

## Consequences
- Behaviour is data in the repository again, and its history is the file's history; but now each step's prompt and tools are a mode, so a workflow stays a short graph.
- One implementation of the workflow semantics (`ez-workflow`). The UI never re-derives a decision; it asks tracks.
- One new service and schema; tracks needs Secret read access in the sandbox namespace, a wider grant than any service except sandbox-proxy and the operator.
- Saves bypass review and go live at once: a saved workflow starts acting on the next matching event. A repository that wants review on workflow changes protects its default branch against the App and loses in-UI saving until a PR flow is added.
- No imports, phases or shared library: a team that wants the same workflow in several repositories copies the file.
- Test runs cost model usage and sandbox time like real runs; they are started by a person, never by an event.

## Decided in review
- The service is named `tracks`.
- Real runs use the Project owner's Claude token; test runs use the token of the person who started them.
- `command` nodes may run a mise task or a shell string.
- Workflows are live as soon as they are saved on the default branch; there is no enable flag.
