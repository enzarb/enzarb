# 0021. Shared workflow library: steps, prompts and graphs as data, nothing compiled in

## Context
ADR-0020 moved the shape of a workflow into the repo (`.enzarb/workflows/*.yaml`), but the engine still compiles in:
- the agent prompts (`workflows/crates/engine/prompts/*.md`) and the list of named prompts (`machine/prompt.rs`);
- what each step returns: the `decision`/`title`/`body` block, parsed by hand per job kind;
- the context each step gets (`{plan}`, `{ci_failure}`, `{merge_base_sha}`, …);
- the default graphs (embedded fallbacks are planned in #215).

The UI hard-codes engine knowledge too. #225: "a plan" was found by job kind and is now read from `state_data.plan`.

Projects cannot change a step's prompt, define their own steps, or share steps across projects. Every new step type is a release.

## Decision

### 1. Steps are files
A **step** is a reusable node template: `steps/<name>.yaml` plus a prompt file.

```yaml
# enzarb/workflows: steps/plan.yaml
version: 1
name: plan
type: agent                  # agent | command | action
prompt: {file: prompts/plan.md}
inputs: [issue, thread, dependencies, triage_summary]   # context the prompt may use
output:                      # JSON Schema of the agent's result block; the kernel validates it
  type: object
  required: [decision, body]
  properties:
    decision: {enum: [plan, questions, blocked]}
    title: {type: string}
    body: {type: string}
artifacts:                   # what the task keeps and the UI can show, by name
  - {name: plan, title: Plan, format: markdown, from: body}
```

- **Prompts** are Markdown with `{placeholder}`s. The placeholders are the step's declared `inputs`, rendered from one generic context object. Context keys are data, documented in `contracts.md`; no prompt is compiled in.
- **Output.** The kernel extracts the agent's trailing JSON block and validates it against `output`. An invalid result fails the node with the schema error, and the recovery run gets that error. Graph conditions read the validated output.
- **Artifacts** are copied into `state_data.artifacts[name] = {title, format, content, node, at}`. The UI lists them generically ("View Plan", or any title) and knows nothing about plans or reviews.
- `command` and `action` steps work the same way: a reusable `run:` or `action:` with declared inputs and outputs.

### 2. Graphs import from libraries
A definition imports repositories under a short alias and uses their steps and graphs:

```yaml
version: 1
name: issue
imports:
  std: enzarb/workflows@v1          # tag, branch or sha
on: [issue_opened]
start: gate
nodes:
  gate: {type: wait, events: [comment, labeled], ...}
  plan: {uses: std/steps/plan, next: post_plan}
  implement:
    uses: std/steps/implement
    prompt: {file: .enzarb/prompts/implement.md}   # per-project override
    next: {pushed: open_pr, else: failed}
  lint: {uses: .enzarb/steps/lint}                  # a step from this repo
```

- **Overrides.** A node's own fields override the step's (`prompt`, `next`, `max_visits`, `optional`, `inputs`). `output` and `artifacts` can only be extended, never loosened.
- **Whole graphs.** A definition can be a single `extends: std/graphs/issue` with node overrides, or use any graph's nodes. Today's graphs become `enzarb/workflows: graphs/issue.yaml` and `graphs/change_request.yaml`.
- **Local steps.** Paths starting with `.enzarb/` refer to the project's own repository.

### 3. Resolution and trust
- **Pinned.** When a task starts, the kernel resolves every import to a commit sha (via the connector, at the definition's default-branch head), reads the referenced steps, prompts and graphs at those shas, validates the result, and pins all of it in `state_data.graph` for the task's life (as ADR-0020 does for the definition). Retry re-resolves.
- **Allow-list.** Imports may come only from the Project's own repositories and the admin-configured library repositories (Helm `workflows.libraryRepos`, default `["enzarb/workflows"]`). Only content at a resolved commit is read, so a library change takes effect only when merged and re-referenced, or when a task starts against a moving ref like `@v1`.
- **The App reads libraries** with the same installation; a library repository must have the enzarb GitHub App installed.
- **Unchanged invariants:** the merge gate (only a wait on exactly `merge_requested` leads to a merge), write-access checks on waits, and the force-push and retarget lease checks stay in the kernel and executor. A step cannot weaken them.

### 4. Nothing compiled in
- The engine ships no prompts and no graphs. A repository without `.enzarb/workflows/<name>.yaml` uses the configured default (Helm `workflows.defaultWorkflows`, e.g. `{issue: "enzarb/workflows/graphs/issue@v1", change_request: …}`).
- If no default is configured, or it can't be read, the task fails visibly with that reason. It never falls back silently.
- `engine/prompts/`, `named_prompt` and the per-kind result parsers are deleted once the library carries them. The compiled machines go with #215.
- The kernel keeps only node types, generic waits, events and actions, the agent runner, schema validation, artifacts and the trust invariants.

### 5. The `enzarb/workflows` repository
- **Layout:** `steps/`, `prompts/`, `graphs/`, `schemas/` (shared output schemas), `tests/` (fixtures run by the graph crate's validator) and a CHANGELOG.
- **Releases:** semver tags (`v1.2.0`) plus moving major tags (`v1`).
- **CI:** it validates every file with the `enzarb-graph` validator, the same crate the engine uses, built from a pinned enzarb release.
- **Its own workflows:** its issues and PRs run on its own graphs.

## Consequences
- **Prompts and steps** change by PR in a library or project repository, with history, review and pinning. Projects share and override them.
- **The UI** shows artifacts and declared actions generically, so the #225 class of hard-coded engine knowledge goes away.
- **One more repository** to version and keep compatible. A library change can affect every project on a moving tag, so breaking changes need a major tag.
- **Migration:**
  1. Extract the current prompts, result contracts and graphs into `enzarb/workflows@v1`, with parity tests that they produce the same prompts and transitions as today.
  2. Add imports, step resolution, schema validation and artifacts to the kernel.
  3. Point the defaults at the library and switch `.enzarb/workflows/*` to `imports`.
  4. Delete the compiled prompts and parsers.
- **#215** changes: no embedded default graphs; its parity work (triage, stacking, mechanical check, mentions) is written as library steps and graphs.
