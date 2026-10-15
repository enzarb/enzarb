# 0020. Graph workflows: definitions in the repo, a runner in the sandbox, a thin kernel outside

## Context
The `issue` and `change_request` workflows are compiled state machines (`workflows/crates/engine/src/machine/`, about 3,000 lines plus a 2,800-line test file). Every behaviour change is an engine change and a release, and every project gets the same behaviour. Most open issues in October 2026 (#163, #165, #176, #182, #192) are changes to *when* a step runs or *what context it gets*, not to the engine itself (#191).

sandbox-init has also been learning about specific tools (npm, mise, Postgres in `prepare.rs`) and enforcing a command policy inside a box whose isolation is already the security boundary. A repo-specific `mise.toml` broke every run in this repo on 2026-10-15 (#182).

We want:
- workflows a project can tailor, versioned in its own repo;
- sandbox-init to be a generic executor;
- no new external orchestration system.

## Decision

### 1. A workflow is a graph
A definition is YAML, `.enzarb/workflows/<name>.yaml`:

```yaml
version: 1
name: issue
on: [issue_opened]            # triggers that create a task
start: prepare
nodes:
  prepare:   {type: command, run: "mise run prepare", optional: true, next: implement}
  implement: {type: agent, prompt: implement, next: {pushed: open_pr, else: failed}}
  open_pr:   {type: action, action: open_change_request, next: done}
  failed:    {type: end, status: failed}
  done:      {type: end, status: done}
```

Node types:

| Type | Runs | Meaning |
|---|---|---|
| `command` | sandbox | Shell command in the run's worktree. Output: `exit_code`, `stdout` tail. `optional: true` continues on failure. |
| `agent` | sandbox | An agent run with a named or inline prompt. Output: the agent's structured result (`decision`, `pushed`, `head_sha`, …). |
| `wait` | kernel | Suspends until one of the listed events (`comment`, `approval`, `ci`, `review`, `merge_requested`, `dependencies_done`, `timer`) arrives. |
| `action` | kernel | A privileged connector call: `post_comment`, `open_change_request`, `merge_change_request`, … |
| `end` | kernel | Terminal, with `status`: `done`, `failed` or `cancelled`. |

- **Edges.** `next` is either a node id or a map from a condition to a node id, checked in order, with `else` as the fallback. A condition is a small expression over the outputs of the node that just ran (`pushed`, `exit_code == 0`, `decision == "approve"`), never arbitrary code.
- **Loops.** A loop is an edge back to an earlier node. Every node has `max_visits` (default 5). Going past it takes the node's `on_limit` edge, or ends `failed`.
- **Validation.** Validation runs before a definition is accepted. It checks:
  - that `start` and every edge target exist;
  - that every node can reach an `end`;
  - that there are no unknown types or actions;
  - that a merge action is only reachable through a `wait` on `merge_requested` (§3).

### 2. The runner lives in the sandbox
- **Segments.** The kernel starts an agent job as a **segment run**: the additive `RunStart.graph` field carries the definition, the node to start at and the outputs so far.
- **What sandbox-init does.** sandbox-init walks the graph itself. It executes consecutive `command` and `agent` nodes in the same worktree, with no round trip per step. It stops at the first `wait`, `action` or `end` node.
- **What it reports.** The additive `RunFinished.graph` returns the node it stopped at and every node's output.
- **Pure core.** The graph logic (schema, validation, conditions, `advance`) is one pure crate (`workflows/crates/graph`). The kernel and sandbox-init both use it as a path dependency, so both sides advance the graph the same way.
- **What sandbox-init stays.** It remains a generic executor: it runs commands and agent runs and owns git and credentials. Tool-specific preparation becomes a `command` node that the repo owns (`mise run prepare`). The legacy `prepare.rs` stays only for legacy-engine runs until those are retired.

### 3. The kernel stays outside, and owns trust
The `workflows` service keeps what an isolated, agent-writable box cannot own:
- **Durable position.** `state_data.graph`: the pinned definition and its sha, the current node, outputs and visit counts. Tasks wait for days.
- **Event routing.** Webhooks, UI actions and timers resume the task waiting on them.
- **Privileged actions.** `action` nodes use the GitHub App credential, which never enters the sandbox.
- **Gates.**
  - The merge invariant from `docs/workflows.md` still holds. Merging needs a human `merge_requested` and a passing readiness check, and validation rejects any graph that reaches `merge_change_request` another way.
  - Approval waits need write access, as today.
- **The definition is trusted only from the default branch.** The kernel reads it through the connector at the default branch's head when a task starts, and pins it for the task's life. An agent can propose a workflow change only through a PR that a human merges.

So an agent with full control of its sandbox can change what happens *inside* a segment, but it can't skip a wait, a gate or an action.

### 4. Opt-in and coexistence
- A repository uses graph workflows when `.enzarb/workflows/<workflow>.yaml` exists on its default branch. Otherwise it keeps the compiled machine.
- Existing tasks keep the engine they started with (`work_items.workflow` = `graph:<name>`).
- The compiled `issue` and `change_request` machines are retired once their behaviour is expressed as built-in default graphs. That is a follow-up, not part of this ADR's first slice.

### 5. First slice
- The graph crate.
- The segment run in sandbox-init (`command` and `agent` nodes).
- The kernel workflow kind with `wait`, `action` and `end` nodes, where actions are `post_comment` and `open_change_request`.
- Definitions read from the repo.
- An `issue` graph for this repo: prepare, implement, open PR.

## Consequences
- **Behaviour becomes data.**
  - A project changes its workflow with a PR, and its history is the workflow's history.
  - Most of today's issues become edits to the default graph.
- **One fewer round trip per step.** Prepare, implement, check and self-fix run back to back in one sandbox run.
- **Two engines for a while.** Both are tested; the compiled machine shrinks as default graphs replace it.
- **The kernel must stay small and generic.** A request for workflow-specific logic in the kernel is a request for a new node type or action instead.
- **A UI graph editor comes later.** It saves by opening a PR against the definition file.
