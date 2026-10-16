# 0022. Workflow engine rewrite: a generic interpreter, every workflow in YAML

Status: Proposed (2026-10-16). Supersedes [ADR-0020](0020-graph-workflows.md) and [ADR-0021](0021-shared-workflow-library.md); amends [ADR-0008](0008-postgres-workflow-engine.md).

## Context
enzarb has two engines side by side:
- the compiled `issue`, `change_request`, `mention` and `stack` machines;
- the graph kernel from ADR-0020.

Both still compile workflow knowledge into Rust, SQL and TypeScript:
- the kinds (`workflow = 'issue'`), states and wait reasons;
- the prompts and their context, and the per-kind result parsers;
- the merge and dependency rules;
- the board columns.

The lessons of October 2026 (see the issues closed for this rewrite: #215, #217–#219, #222, #223) all come down to one cause: **the same rule lives in several places, and copies drift.** Examples:
- queries missed `graph:*` kinds;
- dependency completion was defined three times;
- the UI re-derived engine decisions (#225, #228);
- sandbox-reported facts were trusted (#192);
- hand-synced lists broke releases (#214).

Goal: **the engine contains no workflow-specific knowledge.** Every step, prompt, gate, state name, event alias, board column and runtime is YAML in a shared library ([`enzarb/workflows`](https://github.com/enzarb/workflows)), and projects inherit or override it. The long-term aim is self-improving software. Gates, including merges, are policy a workflow chooses, not engine rules.

## Decision

### 1. Model
- **A task is one running instance of one workflow,** from trigger to end. A task spans an issue's whole lifecycle: issue → PR(s) → merge → release.
- **Subjects:** the GitHub objects a task touches (issue, PRs, comments) are **subjects** linked to the task.
  - A PR that references an issue (a closing keyword or GitHub's Development link) is linked to that issue's task automatically, or to all of them if it references several.
  - A PR that references no issue starts its own task through a binding.
  - An object belongs to at most one live task.
- **Routing:** an event about a linked subject goes to its task, and the task's current wait decides whether it's used. Otherwise it's recorded and ignored. An event about an unlinked object goes through the bindings (§4).
- **Engine phases:** the engine knows four, `running`, `waiting`, `ended(<status>)` and `paused`. Every named state ("Awaiting approval", "Blocked", "Ready to merge", "Released") is YAML: a node plus its declared `reason`, `text` and `column`. End statuses are YAML-defined strings.

### 2. Workflows are lists of phases
- **A workflow** is an ordered list of **phases**:

  ```yaml
  version: 1
  imports: {std: enzarb/workflows@v1}
  phases:
    - {use: std/phases/dependencies}
    - {use: std/phases/start_gate}
    - {id: plan, use: std/phases/plan, with: {prompt: {file: .enzarb/prompts/plan.md}}}
    - {use: std/phases/implement, with: {max_rounds: 5}}
    - {use: .enzarb/phases/lint}
    - {id: review, use: std/phases/review, exits: {rejected: plan}}
    - {use: std/phases/merge, with: {mode: auto_when_green}}
    - {use: std/phases/release, with: {when: [github.release.published]}}
  ```

- **A phase** is a self-contained subgraph with typed **params**, named **exits** and private internals. Its node ids are not a contract.
  - `done` flows to the next phase. Other exits are mapped with `exits:`, or end the task with that status.
  - Exits may target an earlier phase. Each phase has `max_entries` (default 5) and an `on_limit` exit.
  - Re-entering a phase starts it fresh, but the task's outputs, artifacts and facts persist.
  - Phases can nest. To change something a phase doesn't expose as a param, copy the phase into the repo.
- **Nodes inside a phase:**

  | Node | Runs where | What it does |
  |---|---|---|
  | `step` (agent or command) | sandbox | One unit of work |
  | `wait` | engine | Waits for events; it may declare `reason`, `text`, `column`, `actions` (UI buttons that post `/commands`) and `steer` |
  | `action` | engine | A generic connector call |
  | `end` | engine | Ends with a status |

  Edges are conditions over outputs and facts. Loops are capped by `max_visits` and `on_limit`.
- **Steps, phases, runtimes, boards and bindings** are importable files from any allow-listed repo (§8), resolved to a commit sha.

### 3. Prompts and context
- **Prompts are [MiniJinja](https://github.com/mitsuhiko/minijinja) templates** over a documented **task document**:
  - subjects (issue and PRs with their content);
  - the triggering event;
  - each node's validated outputs and artifacts;
  - facts: CI with failing checks and log excerpts, reviews, mergeable, dependencies;
  - relations;
  - history (§10).
- **Steps declare typed `inputs`.** An undefined variable is a validation error.
- **Context the engine doesn't hold** (merge base, affected components, …) comes from earlier `command` steps whose JSON output later steps read. The engine compiles in no context.

### 4. Bindings and layering
- **Bindings map triggers to workflows,** in three layers that override each other in order:
  1. the library default `bindings.yaml`;
  2. the Project setting `spec.workflows: {library, bindings}`;
  3. a repo's `.enzarb/workflows.yaml`.
- **Bindings can filter** (`if:` over the event), and cron schedules are triggers too.
- **Running tasks auto-upgrade:** at each wait and before each step, the engine re-resolves the definition (repo YAML at the default-branch head, libraries at their ref) and continues from the same node id. If that node no longer exists, the task stays pinned and records why. Pinning an exact sha or tag opts out. A running step always finishes on the definition it started with.

### 5. Events
- **Typed catalog:** a documented catalog of standard event objects (`Comment`, `Label`, `Push`, `PullRequest`, `CiStatus`, `CheckRun`, `Review`, `Release`, `Tag`, `Package`, `Deployment`, `WorkflowRun`, `Schedule`, `Adopted`, `DependencyChanged`, `ChildEnded`, …), one JSON Schema each, with generated reference docs. Connectors map provider webhooks into them.
- **Parsed fields:** comments carry parsed `command`/`args` (`/word`, optional leading `@bot`).
- **Custom events:** a project declares them with a schema in YAML and sends them through a signed `POST /v1/events/<project>`. Payloads are validated, documented and visible in the event inspector.

### 6. Execution
- **One interpreter.** `(definition, state, event) → (state, effects)` is a pure, deterministic interpreter in one crate, shared by the engine, `enzarb-lint`, scenario tests, replay and the UI (as WASM).
- **The ADR-0008 reducer stays:** an inbox with dedupe keys, one event per transaction, a row lock per task, and effects queued in the same transaction. The projection (§9) is updated in that same transaction.
- **Effects are generic:** run a step, call an action, refresh a fact, set a timer, create relations. Every result comes back as an event.
- **History is append-only:** events, step runs, results and session events. Task state can be rebuilt by replay.
- **The engine is the only interpreter.** Each step is one call to sandbox-init in the task's persistent workspace. sandbox-init knows nothing about graphs or templates.
- **Workspaces:** a task has a primary workspace (the triggering repo) and may declare more (`workspaces: [{repo, mode: read|write}]`). Each write workspace gets its own branch, push and PR, linked as a subject, and the YAML merges them together.
- **No timeouts or budgets,** because killing a run re-spends it. Instead:
  - **stall detection** (no activity for a long time) **pauses** the task and notifies;
  - **cost is reported** per step and task;
  - **interrupted steps resume** the same agent session (ACP `session/load`) after restarts and sandbox rolls, and never restart unless the YAML's `retry` allows it.
- **Steering:** events listed in a wait's or step's `steer` are delivered into a running agent session as user turns, and `/stop` ends a step with exit `interrupted`.
- **Scheduling:**
  - **capacity** is admin config: concurrent sandbox steps per Project, sandbox size;
  - **`priority`, `concurrency: {group, max}`, `model` and `retry`** are YAML;
  - **one queue and one backoff implementation.**

### 7. Results and facts
- **Facts are verified.** The engine records facts (pushed, head, diff, CI, reviews, mergeable, merged, contained-in-tag) only from git and GitHub, never from agent output.
- **Step outputs come from side effects first:** git state, plus declared output files, e.g. `outputs: {plan: {file: .enzarb/out/plan.md, artifact: Plan}}`. A step that needs data that isn't a file declares `result: {schema}`, and only then is it offered an MCP tool, `submit_result`, typed with that JSON Schema and validated at call time. There is no text parsing and no recovery runs.
- **Artifacts** (title, format, content) are kept per task, and the UI renders them generically.

### 8. Trust
The engine enforces only these:
- **Privileged calls stay in the engine.** GitHub calls use the App's credentials, and every step gets a short-lived token with exactly the permissions its YAML requests (`github: {contents: write, actions: read}`).
- **Facts are verified (§7).**
- **History is rewritten only through the lease-checked retarget action.**
- **Definitions** count only from the default branch, or from an allow-listed library at a resolved commit. The allow-list is the Project's repos plus `workflows.libraryRepos`.
- **Named secrets** live at user, Project and instance scope, encrypted in Postgres and managed in the UI. YAML declares them by name, they're injected only into the requesting step's process and redacted in captures, and a missing secret makes the step wait on a human.

Every gate, merge included, is workflow policy.

### 9. Projection and UI
- **One projection per task,** maintained by the engine, is the only thing the UI, notifications and API read. It holds:
  - title, subjects, phase and node;
  - status, column, `waiting_on`, reason and text;
  - actions (buttons), artifacts, `needs_you`, cost and updated time.
- **No workflow logic in `ui/`.**
- **The board** is a layered `board.yaml` (ordered columns with title and limit). Nodes declare `column:`, and the engine writes the resolved column.
- **Navigation:**
  - each Project gets a **Tasks** tab with its board;
  - the top-level page shows **Waiting on you** across all projects;
  - the workflow **viewer** is read-only for now: the resolved phases, params, exits and origin badges, plus lint.
- **The session inspector** shows each step's full captured session.

### 10. Sessions, metrics, history
- **Session capture:** every agent step streams its full session (prompts, steering, assistant messages, tool calls and results, thinking if exposed, usage) as append-only `session_events` rows (jsonb, partitioned by month, a capped payload with head and tail, a GIN index). Rows are redacted at capture in sandbox-init, retained per Project (default forever) and visible to the Project.
- **Step metrics** are first class per step run: wall, active and queue time; runtime and model; tokens and cost; attempts, resumes, stalls and steering; exit; and attributed sandbox utilization. The Metrics tab aggregates by workflow, phase and step name.
- **History as a data source:** steps can read a bounded summary of recent tasks (`tasks(project, since)`). That powers a library **retrospective** workflow, run on a schedule, which proposes workflow changes as PRs.

### 11. Runtimes
- **Runtime files:** named agent runtimes are library YAML that declares:
  - mise installs (e.g. `npm:@agentclientprotocol/claude-agent-acp@…`);
  - the launch protocol (`acp` or a simple stdio-JSON protocol);
  - capabilities (sessions, resume, steering, interactive) and the secrets it needs.
- **Steps choose one** with `agent: {runtime, model}`.
- **sandbox-init knows only the protocols and mise,** and the sandbox image shrinks to the base OS, mise, git and the shims.

### 12. Chat
Chat stays a **direct, resumable ACP session** for full Claude Code fidelity. It is not a workflow. It shares runtimes, workspaces, secrets and session capture with the engine, and resumes across sandbox restarts and browser reconnects.

### 13. Tooling
- **`enzarb-lint`** (a CLI from the interpreter crate, a GitHub Action `enzarb/workflows/lint@v1`, and WASM in the viewer) validates `.enzarb/` and libraries: schemas, resolution, template variables, reachability, loop caps.
- **YAML scenario tests:** events plus stubbed step results, with expected paths and projections.
- **Replay of recorded tasks** against a new definition comes later.

## Scope of the rewrite (in place)
- **`workflows/`:** rewritten around the interpreter crate.
  - **Deleted:** the compiled machines, the graph kernel, `engine/prompts`, `PromptContext`, the result parsers, recovery runs, the HTTP callback, and the `restart`/`session_limit` special cases.
  - **New schema:** `tasks`, `subjects`, `relations`, `events`, `step_runs`, `session_events`, `facts`, `secrets`, `projection`. The old `workflows`/`workflows_api` objects are dropped, with no migration.
- **`connectors/`:** the event catalog and generic actions.
- **`sandbox-container/`:** a generic executor (command and agent steps; steer, stop and resume; session streaming; git with leases; workspaces; mise). `prepare.rs`, the segment runner and callback delivery are deleted.
- **`proto/`:** new step frames, with the old run frames removed.
- **`ui/`:** renders the projection, the board, the inspector, the viewer, Chat and metrics. Its classification logic is deleted.
- **operator:** checkpointed sandbox rolls. **realtime:** streams projection and session changes.
- **`enzarb/workflows`:** the event aliases, phases, steps, prompts, runtimes, board, bindings, scenario tests and the lint Action.

## Rollout
- **Built on local branches** (`redesign/engine` here and `redesign` in `enzarb/workflows`) and tested locally, then deployed directly to oldie. enzarb.dev has no customers; it dogfoods itself.
- **First start:** an idempotent adopt pass creates tasks for the open, unworked issues through their bindings (an `Adopted` event).

Milestones:
1. M1: the interpreter, formats and `enzarb-lint`.
2. M2: library v0, today's behaviour as YAML.
3. M3: storage and engine.
4. M4: sandbox executor.
5. M5: the connector event catalog.
6. M6: the UI.
7. M7: local end to end.
8. M8: deploy to oldie and delete the old engine.

## Consequences
- **The engine stays small and generic.** A request for workflow-specific engine logic becomes a request for a new generic building block, and every behaviour change is a YAML PR.
- **Each rule has one definition,** and the UI renders instead of deciding, so the drift class of bug goes away.
- **One more repository** to version (`enzarb/workflows`), with semver and moving major tags. A breaking library change needs a major bump.
- **Larger Postgres** from session capture (jsonb, partitioned).
- **No compatibility window:** in-flight work at the cutover is limited to adopting open issues.
