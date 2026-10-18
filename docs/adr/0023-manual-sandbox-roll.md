# 0023. Manual sandbox roll: a generation-bound annotation and a 30m drain timeout

Status: Accepted (2026-10-17).

## Context
A pod-shape change (new sandbox image, env, resources) is queued as `status.pendingChange` and applied only on a drain ack, an idle sandbox or the drain timeout (default 2h). A new image therefore took up to two hours to apply. Since #252 a sandbox shutdown interrupts running steps resumably, so a long wait protects little.

## Decision
- **Roll now.** The owner can release the roll from the Project page. The UI sets the Project annotation `enzarb.dev/roll-now-generation` to the `pendingChange.generation` the user saw, after a confirmation dialog warning that sessions and jobs are interrupted. The server re-reads the Project and refuses (409) if the pending generation moved on.
- **Operator.** An exact match with the pending generation releases the roll (`Why::Manual`), ranked after the ack and before idle and timeout. The 120s minimum pod age still applies. Stale, future, non-numeric or `i64::MAX` values are ignored, and new generations start above any ack or roll-now already on the Project, so a leftover value never releases a later episode.
- **Default timeout: 30m** (was 2h). The key, flag (`--sandbox-roll-drain-timeout`) and env (`ENZARB_SANDBOX_ROLL_DRAIN_TIMEOUT`) are unchanged. No Helm key exists or is added.
- **Remaining drain time.** `status.pendingChange` carries an optional `deadline` (`since` + the drain timeout, so it follows the operator's configuration). The UI shows the time left next to "Roll now" and nothing when `deadline` is absent. The roll still waits for the 120s pod age and the 30s requeue.
- **Idle.** Nothing writes the `enzarb.dev/active-sessions` pod annotation yet. An idle sandbox is released through the workflows drain ack: workflows acks on its next tick once no `run_step` job runs.

## Alternatives rejected
- A `spec` field (`spec.sandbox.rollNow`): needs a CRD change and puts a one-shot action into desired state.
- An unbound boolean annotation: could kill a sandbox for a change the user never saw.
- Only lowering the timeout: every roll would kill busy sandboxes sooner, with no way to roll immediately.

## Consequences
An in-use sandbox is still only killed by an explicit owner action tied to the generation they saw, or by the timeout. No RBAC change: the UI already has `patch` on `projects`.
