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
- **Re-entry** starts the phase fresh (visit counts reset); outputs, artifacts, facts and `vars` persist.
- **`params`** (same types as phase params) are set by the binding's `with`. Phase entries pass them down as `{param: <name>}` in `with` and `max_entries`. A workflow with a required param resolves only with a value (`resolve_with`); lint uses placeholders.

### Task-wide handlers

```yaml
on:
  - {events: [{event: comment, from: writer, if: "event.command == 'cancel'"}], do: cancel}
  - {events: [comment], if: "event.command == 'replan'", do: "goto:plan"}
  - {events: [{event: label, if: "event.label == 'halt'"}], do: stop_step}
  - {events: [{event: issue, if: "event.action == 'closed'"}], do: "end:closed"}
  - {events: [comment], if: "event.mentioned", set: {mentions: "(vars.mentions or 0) + 1"}}
```

- Tried in order on every catalog or custom event, in any phase, at any wait, while a step or action runs and while paused. The first handler whose matchers (and `if`) match wins. `params` is the workflow's params.
- **`do`:** `cancel` ends the task with status `cancelled`; `end:<status>` ends it with that status; `goto:<phase>` enters that top-level phase (it counts as an entry); `stop_step` stops the running step (it finishes `interrupted` and its edges decide; without a running step the handler does not match).
- For `cancel`, `end` and `goto`, a running step first gets `stop_step`; its late result, and a pending action's, are stale and ignored. The event becomes `event`; any timer is dropped.
- **`set`** assigns task variables before `do`. A handler with only `set` does not consume the event: it is still offered to the step's stop/steer matchers or the wait.

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

Each node has exactly one of `step`, `wait`, `action`, `end`, `phase`, `decide` (a node with only `set` is a decide node), and (except `end`) a `next`.

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
- **`decide: {}`** has no effect: on entry its `next` is followed at once (it must end with `else`; no match pauses the task).
- **`action: {name, args, relate, as, subject}`** is a generic connector call. The node continues when the action finishes (`last.ok`, `last.result`, `exit` = `success`/`failure`).
  - **`args`** is any YAML. String leaves are templates; a leaf that is exactly one `{{ expr }}` keeps the expression's JSON type (`number: "{{ subjects.issue.number }}"` is a number, `labels: "{{ outputs.labels }}"` a list); `{param: x}` anywhere is the param's value.
  - **`relate: {kind, targets}`** creates relations before the call (`targets` is an expression yielding a list). **`relate: {kind, from}`** instead evaluates `from` after a successful call, with `result` in scope, yielding one target or a list (`from: "{'repo': result.repo, 'number': result.number}"`). Either way the interpreter emits `create_relations` with the concrete targets.
  - **`as: <name>`** stores a successful call's result as `outputs.<name>`.
  - **`subject: true`** links the object the call returned as a subject: the result must have `kind` (`issue`/`pull_request`) and `number`, and may have `repo` (default `task.repo`), `title`, `url`. **`subject: {kind, number, repo}`** gives `number`/`repo` as expressions over `result` (defaults `result.number`, `result.repo`). The interpreter emits `link_subject {kind, repo, number}` and adds the subject to the document (a pull request is appended; an issue is set if the task has none).
  - On failure (`ok: false`) none of `as`, `subject`, `relate.from` apply.

### Task variables

```yaml
  init:
    set: {rounds: 0, reviewer: "subjects.issue.author"}   # a node with only `set` is a decide node
    next: work
  work:
    step: {use: std/steps/implement}
    set_on_exit: {rounds: "vars.rounds + 1"}
    next: [{if: "vars.rounds < params.max_rounds and last.exit != 'success'", to: work}, {else: done}]
```

- **`set`** (on entry, after the visit cap check) and **`set_on_exit`** (when the node is left, after its edge was chosen and before the target is entered; `last`, `exit` and, at a wait, the accepted `event` are in scope) assign `vars.<name>`. Handlers have `set` too.
- Values: a string is an expression; any other YAML value is a literal; `{param: x}` is the param's value as a literal. Assignments run in order and each sees the ones before it.
- `vars` is task-scoped: it persists across phases and phase re-entry and is readable as `vars.*` in every template and condition. Names are identifiers (`[A-Za-z_][A-Za-z0-9_]*`). An error in an expression pauses the task at the node.

### Templated literals

These fields accept `{param: <name>}` and templates, so phase params drive them:

| Field | Accepts | Rendered |
|---|---|---|
| `max_visits`, phase entry `max_entries` | integer, `{param}`, template over `params` | at resolution (an integer ≥ 1) |
| wait `timer.seconds` | integer, duration (`90s`, `15m`, `2h`, `14d`, `1w`, `1h30m`), `{param}`, template | at resolution when it reads only `params`, else on entry |
| wait `reason`, `text`, `column`, action button `label`/`command` | string (template), `{param}` | in the projection, on every update |
| step `agent.model` | string (template), `{param}` | when the step starts |

- Lint renders param-only templates with the actual (or placeholder) params and checks the type; a template for `max_visits`/`max_entries` that reads anything but `params` is an error. A literal `column` must name a board column, a literal `command` must be one word without `/`.
- An empty rendered `reason`/`column` is unset (the board default applies); a button whose command renders empty is dropped.
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
- **Re-rendering:** while the task stays at a wait, any event that changes what its `reason`, `text`, `column` or buttons render to (a `fact_updated`, a handler's `set`) emits `update_projection` without re-entering the node. Nothing is emitted when the rendering is unchanged.

### Commands

A command is `/name` in a comment (or the UI button that posts it). The kernel only provides the blocks; what a command *does* is declared in YAML.

- **Workflow `commands:`** are defaults every wait inherits: `{name, label?, description?, from?: writer|anyone (default writer), to}`. `name`, `label` and `description` are text fields (`{param: x}` works).
- **Wait `commands:`** adds to or overrides the defaults by `name`. Without `to` the wait's own `next:` rules route the command and the entry only lists it.
- **`to`** is a node of the wait's phase, `exit:<name>` (leave the phase), `start` (the phase's first node), `self` (the wait again) or `goto:<phase id>` (enter a top-level phase). A default whose `to` cannot route in a phase (a node or exit that phase lacks) is skipped there.
- **Order:** the wait's `events:` + `next:` rules run first; a command they leave unrouted goes to its declared route, if the sender may use it (`from: writer` needs `actor.can_write`). `/cancel` is engine-native for writers and ends the task.
- **Commands the `next:` rules match** (`event.command == 'x'`, `== params.p`, `in [...]`) are listed automatically, so libraries that declare no `commands:` keep working.
- **Reply:** a comment with a command the active wait does not take gets one reply from the bot: `` `/x` does not apply right now. Accepted here: ... ``, or "needs write access" when the sender may not use it. Bots, plain text and tasks that are not waiting get none.
- **Projection:** `commands: [{name, label, description?, from}]` lists the commands that apply now (the wait's, its buttons, and `cancel`).
- **Lint:** once the workflow declares `commands:` (or a wait does), every wait must accept `retry` (an error; a warning for a library that declares none). A declared command needs a route that exists; a `from` other than `writer`/`anyone` is rejected.
- **Feedback:** `subjects.*.feedback` holds the comments by people that are not commands and are newer than both `vars.feedback_since` and the subject's `linked_at` (set when the engine opens a pull request for the task). A feedback round sets `feedback_since` when it ends.

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
workspaces:
  - repo: "{{ task.repo }}-docs"
    mode: write
    branch: "enzarb/{{ task.id }}"       # write workspaces only; a template
    base: "{{ task.default_branch }}"    # ref to branch from / check out; a template
github: {contents: write, pull_requests: write}
secrets:
  - ANTHROPIC_API_KEY                    # = {name: ANTHROPIC_API_KEY, scope: project, env: ANTHROPIC_API_KEY}
  - {name: npm_token, scope: user, file: .npmrc}
  - {name: SIGNING_KEY, scope: instance, env: GPG_KEY}
priority: 10
concurrency: {group: "{{ task.repo }}", max: 1}
retry: {max: 2, on: [failure]}       # extra attempts; default `on` is [failure]
platform_wait:                         # ADR-0026: wait out an environment failure, then resume the same run
  - kind: disk_full                  # disk_full | oom | network | load
    until: {platform: {fact: sandbox.disk_pct, op: lt, value: 85}}
steer: [comment, review]             # delivered into the running session
stop: [{event: comment, from: writer, if: "event.command == 'stop'"}]  # this is the default
```

A step that lists a `platform_wait` kind sends that environment failure to the wait before any edge, `retry`, `set_on_exit` or `max_visits` applies; the task stays `running` and the node does not move. What stays on edges: a kind not listed (`toolchain_missing` and `unknown` cannot wait; the library also leaves `network` and `load` out, see gaps.md #22), and a failure with no `environment`. A wait is level-triggered and has no timeout. The standard library opts in `disk_full` and `oom` on every agent step.

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
- **`secrets`** (steps and runtimes): a bare name is a project-scope secret injected as the env var of that name; the full form is `{name, scope: user|project|instance, env: VAR | file: path}` (one of `env`/`file`; neither means `env: <name>`). `file` is relative to the workspace root, without `..`. `run_step` carries the step's secrets normalized; the runtime's are in its file.
- **Workspaces:** `branch` and `base` are rendered into `run_step.workspaces[].branch/base`; `branch` on a read workspace is an error.

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
  - id: stacked
    on: pull_request
    if: "relations | selectattr('kind', 'eq', 'references') | list | length > 0"
    use: std/workflows/change_request
    with: {parent: "{{ relations[0].task }}", number: "{{ event.number }}"}
```

- **Layering** (`merge_bindings`): a binding whose `id` appears in a higher layer is replaced by it; `disabled: true` removes it; bindings without an id are kept.
- **Selection** (`select_binding`): the merged list is ordered repo, Project, library; the first binding whose trigger and `if` match wins.
- **Routing context:** `if` and `with` read only `event` and `relations` (`RoutingContext`). The engine fills `relations` with a summary of the live tasks the event's object is related to (e.g. the task of an issue a new PR references): `{kind, task, workflow, status, phase, node, vars, subject}` each.
- **`with`** gives the bound workflow's `params`. String leaves are templates over the routing context (a sole `{{ expr }}` keeps its JSON type). `select_binding_in(&bindings, &routing)` returns the binding and the rendered params for `resolve_with`. Lint checks `with` keys against the workflow's declared params.

## Checks

Schema: [`schemas/check.json`](schemas/check.json). `checks/*.yaml` in a library (ADR-0024). Each file is one liveness check that the engine's periodic `platform` pass evaluates; while it holds, one **finding** is open. A finding only shows (on the task's `reason`/`text`, in `task_v.findings` and in `finding_v`); it never acts. Thresholds are data: the engine has no defaults.

```yaml
version: 1
id: ci-never-started
kind: dead_wait            # or platform_failure
severity: warning          # info | warning | critical
scope: task                # task (default, per open task) | platform (per Project)
text: "Waiting on CI at {node}, but the head has no check runs"   # {task} {node} {phase} {project}
issue:                     # optional: file one issue when the finding opens
  title: "Task {task} waits for CI that cannot start"
  body: "Task {task} in {project} waits at `{node}` ..."
  labels: [liveness, dead-wait]
  # repo: owner/name       # defaults to the task's repo; required for scope: platform
when:
  all:
    - waiting_on: external
    - fact_count: {name: ci.check_runs, eq: 0}
    - any:
        - fact: {name: mergeable, is: unknown}
        - fact: {name: mergeable, is: false}
    - idle_for: {no_event: 30m}
```

`when` is `all`, `any`, `not` over these leaves (no scripting):
- `waiting_on: <x>`, `node: <phase/node>`, `phase: <id>`: the task's projection.
- `fact: {name, is}`: a stored fact. `is: unknown` holds when the fact is missing or null; otherwise `is` is compared with the value. `name` may descend a JSON value with dots (`ci.sha`).
- `fact_unknown_for: {name, refreshes: N}`: the fact was refreshed to null at least N times in a row.
- `fact_count: {name, eq: N}`: a list's length or a number. A missing or null value does not match.
- `idle_for: {no_event: <dur>, no_step: <dur>}`: no event reached the task and/or no step ran for that long (a running step counts as now). Durations: `90s`, `15m`, `2h`, `14d`, `1w`.
- `platform_wait: {kind?}`: the task is in a platform wait (ADR-0026), optionally for this failure kind (`disk_full`, `oom`, ...).
- `command_ignored: {for: <dur>, command: <name>}`: a command (a comment's `/retry`, or one from the UI) reached the task at least `for` ago and nothing has happened to the task since: no change of node or status and no effect. `command` (without the slash) restricts it to that command; any command otherwise. Only the latest command counts.
- `relation_count: {kind, inbound, eq|gt}`: the number of `engine.relations` rows of that `kind` the task holds (`blocked_by`, `child`, ...), or with `inbound: true` the rows other tasks hold towards its subjects (an epic's `parent` rows naming a sub-issue). Exactly one of `eq` and `gt`.
- `platform: {fact, arg, op, value}` (`arg` only for `workflow_run.status`: `<owner>/<repo>:<workflow file>`, e.g. `enzarb/enzarb:release.yml`; `op`: `eq` default, `ne`, `gt`, `ge`, `lt`, `le`): `workflow_run.status`, `chart.deploy_state`, `sandbox.disk_pct`, `sandbox.mem_working_set_pct` or `project.pending_change`. Values: `workflow_run.status` is the conclusion of the latest finished run on the repository's default branch (`success`, `failure`, ...); `chart.deploy_state` is `deployed`, `pending` or `stuck`; `sandbox.disk_pct` and `sandbox.mem_working_set_pct` are percentages (memory is the working set, `memory.current` less the `inactive_file` page cache, over the cgroup limit; it needs the sandbox's cgroup limit, else unknown); `project.pending_change` is a boolean. An unknown platform fact makes the check undetermined: it raises nothing and leaves an open finding as it is.

**`issue:`** (any check kind and scope) files one issue the first time the finding opens: `{title, body, labels?, repo?}`, with the `text` placeholders. The engine queues a `file_issue` job in the same transaction as the finding, keyed `finding:<finding key>`. The key carries no time and is scoped to the repo, so a finding that clears and opens again, or a repeated pass, updates the same issue and never files a second one. `repo` defaults to the task's repo and is required for `scope: platform` (there is no task). Nothing closes the issue when the finding clears.

Finding keys are `<id>:<task id>` (task scope) or `<id>:<project>` (platform scope); two checks on one task give two findings. `enzarb-lint` validates `checks/` (unknown leaves, bad durations, task leaves in platform scope).

## Templates and conditions

- **One language:** prompts, `text`, action `args`, workspace repos, `concurrency.group` and `command.run` are [MiniJinja](https://docs.rs/minijinja) templates; `if` conditions are MiniJinja expressions over the same context.
- **The context:** the [task document](task-document.md) roots (`task`, `subjects`, `event`, `outputs`, `artifacts`, `facts`, `relations`, `history`, `vars`, `instance`), plus `params`, `last`, `exit`, and for steps their declared `inputs` by name. Action `relate.from` and `subject` expressions also see `result`.
- **`instance`** is `{bot: {login, handle}, url}`, filled by the engine. Comment events carry `mentioned: true` when the body mentions `@<instance.bot.handle>`.
- **Filters and tests** besides MiniJinja's built-ins (`lower`, `upper`, `default`, `join`, …):
  - `tojson`;
  - `x | shell_quote`: one single-quoted POSIX shell word;
  - `x | contains(y)` / `x is contains(y)`: substring, list membership or map key (false on undefined);
  - `s | regex_match(re)` / `s is regex_match(re)`: Rust regex, unanchored (a bad pattern is an error);
  - `"14d" | duration`: seconds (`s`, `m`, `h`, `d`, `w`, combinable; integers pass through).
- **Undefined is an error at lint time:**
  - a root that isn't a document root or declared input;
  - `params.<x>` for an undeclared param;
  - an unknown second-level field of `task`, `subjects`, `facts` or `instance` (`vars.*` and `outputs.*` are open).
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
  - `call_action`, `refresh_fact`, `set_timer`, `create_relations` and `link_subject {kind, repo, number}`;
  - `update_projection` (status, phase, node, column, `waiting_on`, reason, text, actions, artifacts, `needs_you`).
- **Handlers first:** every catalog/custom event is offered to the workflow's `on:` handlers before the step or wait (see [task-wide handlers](#task-wide-handlers)).
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
params: {max_rounds: 3}                  # workflow params, as a binding's `with` gives them
document: {subjects: {issue: {repo: acme/app, number: 7, title: Crash on start}}, vars: {seed: 1}}
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
  vars: {rounds: 1}                      # task variables with these values
```

`effects` names include `link_subject`; a `fact` item that only re-renders the wait expects `effects: []` and checks `projection`.

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
