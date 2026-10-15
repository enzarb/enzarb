# 0018. Epics: breaking a big issue into sub-issues

## Context
Some issues are too big for one pull request. Planning and implementing them as one change produces a change request nobody can review. They should be split into sub-issues, automatically when the issue is big enough, and on request.

## Decision
- **Triggers.** Triage has a fourth decision, `epic` ("clear enough to plan, too big for one pull request"). A person with write access can force it with a `/epic` comment line or the `epic` label. The label needs the connector to report label events, so there is a new connector event `WorkItemLabeled` (`data.label`); GitHub reports only the `epic` label. Both are honoured only from someone with write access (the same check as `/approve`); from anyone else the label is rejected and `/epic` is an ordinary comment.
- **Breakdown, then approval.** The plan step of an epic writes a breakdown instead of an implementation plan: a readable list plus one `enzarb-subissues` JSON block (`key`, `title`, `body`, `depends_on`). The engine validates it (at most 8 entries, no cycle, no unknown or duplicate key, non-empty titles). The breakdown is posted as the plan and needs the usual `/approve`. Nothing is created before a person approves it.
- **Phases.** `Decomposing` runs the non-agent job `create_sub_issues`; `AwaitingChildren` waits for the sub-issues. The epic never enters `Implementing` and never opens a change request. It is `Done` when every sub-issue is `Done`.
- **Idempotent creation.** Each sub-issue carries `<!-- enzarb:epic=<epic key>:<key> -->` and is created with the idempotency key `epic-<epic key>-<key>`; the GitHub connector looks for an issue with the key's marker before creating. A retry of a half-finished job resumes without duplicates. A partial failure fails the job; the existing retry action runs it again. The marker lookup scans only the newest issues, so on a very busy repo a retry could miss an earlier child and create a duplicate; this is an accepted limit of the MVP. Failed sub-issue links are logged only (best effort), so the epic shows no hint that links are missing.
- **Linking and order.** Sub-issues are linked with GitHub's sub-issues API, with a task list in a managed section of the parent's body as the fallback. Order is the existing dependency model (ADR-0017): `Blocked by #N` text in the sub-issue body, native "blocked by" relationships best-effort, and rows in `work_item_dependencies` with a new source `epic` that the engine writes itself (the epic waits for each sub-issue; a sub-issue waits for what it is ordered after). The ticker's dependency sweep covers epics in `AwaitingChildren`, so completion needs no special event.
- **Sub-issues are normal issues.** enzarb opens them, and the workflow engine ignores bot-authored events, so this is a deliberate, narrow exception, checked in two places. The webhook receiver passes a bot-opened issue only when its body has an epic marker. The engine accepts it only when the actor is enzarb's own login and the marker names a parent that the reducer found in the same Project, in `Decomposing` or `AwaitingChildren`, with that key in its approved plan. Then the sub-issue runs triage, plan and `/approve` like any issue, so nothing is implemented without a human.
- **Guardrails.** Epics do not nest: the depth of a sub-issue is the stored parent's plus one (never read from the body), and an issue at `max_epic_depth` (default 1) cannot be an epic: triage's `epic` is turned into `ready`, `/epic` and the label are rejected. A breakdown has at most `max_epic_children` (default 8) entries. Both are engine `Config` values with defaults; they are not exposed as deployment settings yet.
- **Closed sub-issues.** If every remaining sub-issue was closed without completing, the epic comments once and waits for a maintainer, who can `/approve` to close it.

Rejected alternatives:
- *A plan variant that creates the issues without a separate approval.* A bot would fan out issues with no human gate.
- *A separate orchestrator workflow type.* Too heavy: two phases on the issue machine are enough.
- *Creating the sub-issues right after triage.* Same missing approval.

## Consequences
- Shared contracts stay additive: a connector event type and `EventData.label`, two issue phases, optional `state_data` keys, a job kind, a dependency source (a migration widens one CHECK constraint). A consumer that does not know them ignores them; a UI that does not know the new phases shows them as active.
- A sub-issue is `Done` when its change request is opened, so the epic is `Done` at that point, not when the changes are merged. Merge order is still enforced by the dependency gate of ADR-0017.
- Closing or cancelling an epic leaves its sub-issues open.
- Not done: UI display of the epic and its sub-issues, re-planning an epic after its sub-issues exist, adding or removing sub-issues later, a `no-epic` opt-out label, sub-issues in other repositories.

## Amendment (2026-10, graph workflows)
With graph workflows ([ADR-0020](0020-graph-workflows.md)) the same behaviour is expressed as nodes: a `create_sub_issues` action (only after a wait on `approval`), `children_done` waits, and the `epic` item fact; see [workflows.md](../workflows.md#graph-workflows). Graph tasks also differ on one point, on request: a sub-issue (any graph task) whose dependencies are not *merged* stays `Blocked`, including for triage and planning, and starts by itself once they are. An open change request does not unblock it. Compiled epics keep the behaviour above.
