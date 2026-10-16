# Workflow definition formats

The authoritative human spec of the files decided in [ADR-0022](../adr/0022-workflow-engine-rewrite.md). The types live in `workflows/crates/flow` (`enzarb-flow`); the JSON Schemas under [`schemas/`](schemas/) and the references [events.md](events.md) and [task-document.md](task-document.md) are generated from them, and a test keeps them current.

The engine knows none of the names used below (`gate`, `plan`, `awaiting_start`, `rejected`, …). They are examples; every phase, step, node, exit, status, reason, column and command is data.

Contents: [references](#references), [workflow](#workflow-file), [phase](#phase-file), [nodes and edges](#nodes-and-edges), [step](#step-file), [runtime](#runtime-file), [board](#board-file), [bindings](#bindings-file), [templates and conditions](#templates-and-conditions), [interpreter semantics](#interpreter-semantics), [scenario tests](#scenario-tests), [enzarb-lint](#enzarb-lint).

## References

- **Imports:** `imports: {std: enzarb/workflows@v1}` maps an alias to `owner/repo@ref`. A file's aliases are its own `imports` plus those of the file that referenced it, if both are in the same repository.
- **A reference** like `std/phases/plan` starts with an alias: the file `phases/plan.yaml` in that repository at that ref. Any other reference (`.enzarb/phases/lint`, `steps/plan`) is a path from the root of the repository of the file that contains it. `.yaml` is appended when the last segment has no extension.
- **Text files** (`{file: prompts/plan.md}`) resolve the same way, with no extension added.
- **Allow-list:** only the task's repository and the allow-listed repositories (the Project's repos plus `workflows.libraryRepos`) may be imported. `..` is not allowed in references.
- **Pinning:** every file used is recorded with the commit sha it was read at (`Resolved.origins`).

## Workflow file

Schema: [`schemas/workflow.json`](schemas/workflow.json).

```yaml
version: 1
description: Issue to release
imports: {std: enzarb/workflows@v1}
board: std/board                      # optional board file
events:                               # optional custom events
  deployed:
    description: The deploy pipeline finished
    schema: {type: object, required: [env], properties: {env: {type: string}}}
phases:
  - {use: std/phases/start_gate}
  - {id: plan, use: std/phases/plan, with: {prompt: {file: .enzarb/prompts/plan.md}}}
  - use: std/phases/implement
    with: {max_rounds: 5}
    exits: {failed: plan}             # backward target
    max_entries: 3                    # default 5
    on_limit: failed                  # default limit_reached
  - {id: review, use: std/phases/review, exits: {rejected: plan, abandoned: {end: closed}}}
  - {use: std/phases/merge, with: {mode: auto_when_green}}
```

- **`id`** defaults to the last segment of `use`; ids must be unique.
- **Exits:** every phase has the implicit exits `done` and `limit_reached`, plus those it declares.
  - `done` goes to the next phase (the last phase's `done` ends the task with status `done`).
  - `exits:` maps an exit to `next`, to any phase id (earlier or later), or to `{end: <status>}`.
  - An unmapped exit other than `done` ends the task with the exit name as its status.
- **`max_entries`/`on_limit`:** entering a phase more than `max_entries` times takes its `on_limit` exit instead (routed like any exit; if it routes back to the same phase the task ends with that status).
- **Re-entry** starts the phase fresh (visit counts reset); outputs, artifacts and facts persist.

## Phase file

Schema: [`schemas/phase.json`](schemas/phase.json).

```yaml
version: 1
description: Wait for a maintainer to start the work
params:
  command: {type: string, default: plan, description: Command word that starts the work}
  prompt:  {type: prompt, default: {file: prompts/plan.md}}
  mode:    {type: enum, values: [auto, manual], default: auto}
  rounds:  {type: integer, default: 3}
  labels:  {type: list, items: string, default: []}
exits: [rejected]
start: gate
nodes:
  gate: {...}
```

**Param types:** `string`, `integer`, `boolean`, `prompt` (inline text or `{file: path}`, resolved in the file that wrote the value), `command` (a command line), `enum` (`values` required), `list` (`items`, default `string`). A param without `default` is required. Unknown `with` keys are errors.

**Passing params down:** inside a phase, any `with` value written `{param: <name>}` is the value of the enclosing phase's param. Templates read params as `params.<name>`.

**Nesting and copying:** a node can run another phase (`phase:` below). To change what a phase doesn't expose, copy the file into the repo and `use` the copy; nothing else is needed.

## Nodes and edges

Each node has exactly one of `step`, `wait`, `action`, `end`, `phase`, and (except `end`) a `next`.

```yaml
nodes:
  implement:
    step: {use: std/steps/implement, with: {prompt: {param: prompt}}}
    max_visits: 5           # default 10, per phase entry
    on_limit: give_up       # node entered instead; without it the phase exits limit_reached
    next:
      - {if: "last.exit == 'success' and facts.git.pushed", to: open_pr}
      - {if: "last.exit == 'interrupted'", to: stopped}
      - {else: implement}
  give_up: {end: {status: failed}}
```

- **`next`** is a node id, or ordered branches: `{if: <expression>, to: <node>}` …, then `{else: <node>}`. The first true branch wins.
  - Step, action and phase nodes must end with `else`.
  - A wait may omit `else`: when no branch matches, it keeps waiting.
- **`end: {status: s}`** leaves the phase through exit `s` (which must be `done`, `limit_reached` or declared).
- **`phase: {use, with}`** runs a nested phase; its exit is `exit` in this node's branches.
- **`action: {name, args, relate}`** is a generic connector call. String `args` are templates. `relate: {kind, targets}` also creates relations (`targets` is an expression yielding a list). The node continues when the action finishes (`last.ok`, `last.result`, `exit` = `success`/`failure`).
- **`step: {use, with}`** or **`step: {inline: <step file body>, with}`** runs a step.

### Wait

```yaml
  approve:
    wait:
      events:
        - {event: comment, from: writer, if: "event.command in ['plan', 'approve']"}
        - {event: label, if: "event.label == 'approved'"}
        - review
      from: anyone              # default for matchers without `from`
      reason: awaiting_approval
      text: "Waiting for a maintainer to approve the plan for #{{ subjects.issue.number }}"
      column: review
      waiting_on: human         # default: human with actions or writer-only events, else external
      actions: [{label: Approve, command: approve}, {label: Reject, command: reject}]
      steer: [comment]          # forwarded into the task's last agent session
      refresh: [ci, mergeable]  # facts refreshed on entry
      timer: {seconds: 86400}   # fires a `timer` event
    next:
      - {if: "event.command == 'reject'", to: rejected}
      - {if: "event.type == 'timer'", to: remind}
      - {else: approved}
```

- **Matchers:** a type name, or `{event, from, if}`. Types are the [catalog](events.md) types, the workflow's declared custom events, `timer` and `fact` (a refreshed fact: `event.fact`, `event.value`).
- **`from: writer`** requires `event.actor.can_write`, which the engine fills from GitHub; it is never taken from a payload.
- **Accepting an event** sets `event` in the document to it (timers and facts don't), then follows `next`.
- **`actions`** are UI buttons; each posts `/command` as the clicking user, so it arrives as a `comment` event like a typed command.

## Step file

Schema: [`schemas/step.json`](schemas/step.json). Exactly one of `agent` and `command`.

```yaml
version: 1
description: Write a plan
params:
  prompt: {type: prompt}
agent:
  runtime: runtimes/claude           # a runtime file reference
  model: opus
  prompt: {param: prompt}            # inline text | {file: path} | {param: name}
inputs:
  feedback: {from: outputs.review.comments, type: list, required: false}
  issue:    {from: subjects.issue, type: object}
outputs:
  plan: {file: .enzarb/out/plan.md, artifact: Plan}    # artifact: also kept as a titled artifact
  meta: {file: .enzarb/out/meta.json, optional: true}  # format from the extension: markdown, json, text
result:
  as: plan_result                    # output name (default: the node id)
  schema: {type: object, required: [children], properties: {children: {type: array}}}
workspaces: [{repo: "{{ task.repo }}-docs", mode: write}]
github: {contents: write, pull_requests: write}
secrets: [ANTHROPIC_API_KEY]
priority: 10
concurrency: {group: "{{ task.repo }}", max: 1}
retry: {max: 2, on: [failure]}       # extra attempts; default `on` is [failure]
steer: [comment, review]             # delivered into the running session
stop: [{event: comment, from: writer, if: "event.command == 'stop'"}]  # this is the default
```

```yaml
version: 1
command:
  run: "mise run affected --json > .enzarb/out/affected.json"   # a template
  tools: {mise: [jq@1.7]}
outputs:
  affected: {file: .enzarb/out/affected.json}
```

- **Inputs** are named template variables taken from the task document (`from` is a dotted path from a document root, or `params.<name>`). A missing required input pauses the task (`resume` retries the node); a missing optional one is undefined. A wrong type pauses too.
- **Outputs** come from side effects: the executor reads the declared files after the step and reports them; they are stored in `outputs.<name>`.
- **`result`** is the only way to return non-file data: the runtime offers a `submit_result` tool typed with `schema` and validates at call time. The result is stored in `outputs.<as>`.
- **Exits** are reported by the executor (`success`, `failure`, `interrupted`, …).
- **`retry`** re-runs the step (same node visit) when it exits with one of `on`.

## Runtime file

Schema: [`schemas/runtime.json`](schemas/runtime.json).

```yaml
version: 1
description: Claude Code over ACP
install: {mise: ["npm:@agentclientprotocol/claude-agent-acp@0.5"]}
launch:
  protocol: acp                      # acp | stdio-json
  command: [claude-agent-acp]
  env: {CLAUDE_CODE_DISABLE_AUTOUPDATE: "1"}
capabilities: {sessions: true, resume: true, steering: true, interactive: true, structured_output: true}
secrets: [ANTHROPIC_API_KEY]
```

## Board file

Schema: [`schemas/board.json`](schemas/board.json).

```yaml
version: 1
columns:
  - {id: triage, title: Triage}
  - {id: doing, title: In progress, limit: 5}
  - {id: review, title: Review}
  - {id: blocked, title: Blocked}
  - {id: done, title: Done}
defaults: {running: doing, waiting_human: review, waiting_agent: doing, waiting_external: blocked, paused: blocked, ended: done}
```

- **Column choice:** a wait's `column`, else the default for the engine phase (running, waiting on human/agent/external, paused, ended).
- **Layering** (library → Project → repo, `BoardFile::layer`): a column with the same id replaces the lower one in place; new columns are appended; each set default overrides.
- **Lint:** when a board has columns, every `column:` and default must name one.

## Bindings file

Schema: [`schemas/bindings.json`](schemas/bindings.json). The library's `bindings.yaml`, the Project's `spec.workflows.bindings` and a repo's `.enzarb/workflows.yaml` have this shape.

```yaml
version: 1
imports: {std: enzarb/workflows@v1}
bindings:
  - {id: issues, on: issue, if: "event.action == 'opened'", use: std/workflows/issue}
  - {id: prs, on: pull_request, if: "event.action == 'opened' and not event.references", use: std/workflows/change_request}
  - {id: retro, on: {schedule: "0 6 * * 1"}, use: std/workflows/retrospective}
  - {id: mentions, disabled: true}            # removes an inherited binding
```

- **Layering** (`merge_bindings`): a binding whose `id` appears in a higher layer is replaced by it; `disabled: true` removes it; bindings without an id are kept.
- **Selection** (`select_binding`): the merged list is ordered repo, Project, library; the first binding whose trigger and `if` match wins.
- **`if`** may only read `event`.

## Templates and conditions

- **One language:** prompts, `text`, action `args`, workspace repos, `concurrency.group` and `command.run` are [MiniJinja](https://docs.rs/minijinja) templates; `if` conditions are MiniJinja expressions over the same context.
- **The context:** the [task document](task-document.md) roots (`task`, `subjects`, `event`, `outputs`, `artifacts`, `facts`, `relations`, `history`), plus `params`, `last`, `exit`, and for steps their declared `inputs` by name.
- **Undefined is an error at lint time:**
  - a root that isn't a document root or declared input;
  - `params.<x>` for an undeclared param;
  - an unknown second-level field of `task`, `subjects` or `facts`.
- **Undefined at run time** (e.g. `outputs.review` before review ran) is falsy and renders empty.
- **Errors** point at the template's file and line when it lives in its own file (also through a `prompt` param), else at the YAML key.

## Interpreter semantics

`enzarb_flow::step(&Resolved, &TaskState, &TaskEvent) -> (TaskState, Vec<Effect>)` is pure and deterministic. Ids (`run-N`, `call-N`, `timer-N`) come from a counter in the state, and time arrives only inside events.

- **Status:** `new`, `running`, `waiting`, `ended {status}`, `paused {reason, previous, resume_at}`.
- **Events in:**
  - `start` and `event` (catalog or custom);
  - `step_finished {run_id, exit, outputs, result, git}`;
  - `action_finished {call_id, ok, result}`;
  - `timer {timer_id}` and `fact_updated {fact, value}`;
  - `stall_detected` and `resume`.
- **Effects out:**
  - `run_step` (rendered prompt or command, runtime, workspaces, GitHub permissions, secret names, priority, concurrency, outputs, result schema);
  - `steer` and `stop_step`;
  - `call_action`, `refresh_fact`, `set_timer` and `create_relations`;
  - `update_projection` (status, phase, node, column, `waiting_on`, reason, text, actions, artifacts, `needs_you`).
- **While a step runs:** an event matching the step's `stop` emits `stop_step`. The executor then reports exit `interrupted`, and edges decide what follows. An event matching `steer` emits `steer`. Other events are ignored.
- **Stale results are ignored:** a result or timer with an unknown id changes nothing.
- **Stall:** `stall_detected` pauses the task and keeps the running step.
  - `resume` restores the previous status.
  - A result that arrives while stalled un-pauses and continues.
- **Errors pause, never panic:** an unrenderable template, a missing input, no matching branch, or more than 1000 engine transitions in one event. `resume` re-enters the node named in `resume_at`.
- **Auto-upgrade:** `reresolve(&new, &state)`:
  - returns `deferred` while a step or action runs;
  - returns `upgraded` when every frame's phase and node exist in the new definition (and a waiting node is still a wait);
  - otherwise returns `pinned {reason}`, sets `state.pinned` and adds a note.
- **Ended tasks** ignore everything.

## Scenario tests

Schema: [`schemas/scenario.json`](schemas/scenario.json). Files under `tests/` (library) or `.enzarb/tests/` (repo):

```yaml
name: an approved issue is implemented and released
workflow: .enzarb/workflows/issue.yaml
document: {subjects: {issue: {repo: acme/app, number: 7, title: Crash on start}}}
start: {type: issue, action: opened, number: 7}
stubs:                                   # step results by phase/…/node or bare node id; the last one repeats
  work/implement: [{exit: success, git: {pushed: true}, outputs: {summary: "Fixed."}}]
  check: [{exit: failure}, {exit: success}]
actions: {comment.create: [{ok: true}]}  # default {ok: true}
events:
  - expect: {at: gate/wait, projection: {reason: awaiting_start, needs_you: true, actions: [Start]}}
  - event: {type: comment, body: /start, command: start, actor: {login: m, can_write: true}}
    expect: {at: release/wait_release, effects: [run_step, run_step, run_step, refresh_fact, set_timer]}
  - step_finished: {exit: interrupted}   # finishes an unstubbed running step
  - {fact: {fact: ci, value: {sha: abc, state: success}}}
  - {timer: true}
  - {stall: true}
  - {resume: true}
expect:
  status: ended:done                     # running | waiting | paused | ended:<status>
  path: [gate/wait, gate/done, …]        # every node entered, exactly
  outputs: {summary: "Fixed."}
```

## enzarb-lint

```text
enzarb-lint check <dir> [--library <alias|owner/repo>=<path>]... [--repo <owner/repo>] [--format text|github]
enzarb-lint test  <dir> [same options]
enzarb-lint schemas <docs/workflows dir>
```

- **`check`** validates every file it finds:
  - workflows under `workflows/` and `.enzarb/workflows/`, resolved fully;
  - phases and steps under `phases/`, `steps/` (and `.enzarb/…`), resolved standalone with placeholder params;
  - runtimes, `board.yaml` and `boards/`;
  - `bindings.yaml` and `.enzarb/workflows.yaml`, with every bound workflow resolved.
- **What `check` reports:**
  - YAML syntax and duplicate keys;
  - schema errors such as unknown fields;
  - references and the allow-list;
  - params;
  - exits and targets;
  - edges and `else`;
  - template and condition variables;
  - event types, facts and columns;
  - unreachable nodes (as warnings).
- **`--library std=../workflows`** serves an imported repository from a local checkout (by alias or `owner/repo`).
- **`--format github`** prints `::error file=…,line=…,col=…::…` annotations.
- **`test`** runs the scenario files.
- **Exit codes:** 0 clean, 1 errors or failing scenarios, 2 usage or IO error.
