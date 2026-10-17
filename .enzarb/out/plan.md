# Plan: transcripts on the task page (#246)

## Summary
Reading the code on `main`, most of this already exists. After ADR-0022 (#229), every agent step of a task streams its full session into Postgres (`engine.session_events`). The task page shows it in a per-step session inspector, live while the step runs and stored afterwards. The triage was written against stale docs (`docs/mvp.md:26`, `docs/gaps.md` #10, `docs/workflows.md:244`, and the `transcript.jsonl` text in `docs/contracts.md`), which still describe the old PVC and object-store plan.

The proposed PR is therefore small. It makes the docs match, closes the test gaps on the task-page transcript path, and fixes whatever a manual check of that path turns up. It adds **no new table, no new frame and no new transport**.

## What already exists (read, not run)
- **Capture:** `sandbox-container/src/steps/mod.rs` sends `Ctrl::SessionEvent` frames for each step. The sandbox redacts secrets before sending (ADR-0022).
- **Persistence:** `workflows/crates/engine/src/executor.rs` (`store_session_events`) inserts each event into `engine.session_events`. The table is partitioned by month, and payloads over 1 MB are capped to head and tail with `truncated = true`. Inserts are idempotent (`ON CONFLICT DO NOTHING`), and a resumed step continues from `max(seq)`.
- **Read path:** `engine_api.session_v(run_id, after_seq, limit)` is called from `ui/src/lib/server/tasks-service.ts`. `getSession` in `ui/src/lib/tasks.remote.ts` wraps it.
- **UI:** `ui/src/routes/(app)/task/[id]/+page.svelte` has a "Steps" section. `StepRuns.svelte` lists each step run, and `SessionInspector.svelte` and `SessionEventView.svelte` render the events. The inspector pages through events, tails them every 2 s while live, and supports search and "Jump to error".
- **Survives sandbox deletion:** the data is in Postgres, not on the PVC.

## Approach
Reuse the existing store and viewer. A new `job_transcripts` table, a separate transcript upload at `RunFinished`, or reusing `ChatTranscript.svelte` would each duplicate this path. I reject all three:
- A second table would store the same data twice and have to be kept in sync.
- `ChatTranscript` parses raw ACP `session/update` records, while step sessions are already normalised kinds (`assistant`, `tool_call`, `tool_result`, and so on).
- An object store (the old plan) is superseded by ADR-0022, and @ruckc asked for Postgres.

The work is in four parts:
1. **Verify behaviour end to end.** Using the dev setup (`docs/dev.md`) or the engine test fixtures, run an issue task through triage, plan and implement. Check that each step's session appears on `/task/<id>`, tails live while the step runs, and is still there after the sandbox is deleted. Also check the graph-task case (`docs/gaps.md` #17, one `run_id` per step run) and an older step with no events, which should show an empty state rather than an error. Anything that fails becomes a fix in this PR.
2. **Make the transcript easier to find, if verification shows it is hidden.** Today it sits behind an expand button per step run in a "Steps" section. If a running step does not open its inspector with `live` on, make it do so. Consider labelling the button "Transcript". Each is a small change in `StepRuns.svelte`.
3. **Tests** (below).
4. **Docs** (below).

## Changes
- `docs/mvp.md:26`: replace "Transcript object storage…" with: the transcript of each step is stored in Postgres (`session_events`) and shown on the task page.
- `docs/gaps.md` #10: resolve it as Postgres (ADR-0022). The open part is retention. Session events are kept forever per Project by default, and monthly partitions allow dropping old ones.
- `docs/workflows.md:244` ("Out of the engine"): the final transcript lives in `session_events`, not in object storage, and `jobs.output` keeps only the summary. Check whether `docs/workflows.md` still describes the pre-ADR-0022 engine elsewhere, but do not rewrite more than this.
- `docs/contracts.md` (~`:129`, `:146`, `:225`): mark the `transcript.jsonl` and `log_channel` run records as the legacy `run_start` path and point to `docs/workflows/contracts-v2.md` for step sessions. This is a docs-only edit.
- `ui/src/lib/components/tasks/StepRuns.svelte`: only the small changes from part 2, if verification shows they are needed.
- Any bug found in part 1, fixed in the owning component. If it is in `workflows` or `sandbox-container`, keep the contract change additive.

## Tests
- `ui`: add component tests for `SessionInspector` (first page, paging at 500, live tail appends only new seqs, search filter, "Jump to error", load failure notice) and `SessionEventView` (each kind and `truncated`). Add a `StepRuns` test: running step vs ended step, and no events shows an empty state. The only existing test near this is `tasks.svelte.test.ts`.
- `ui/src/lib/server/tasks-service.spec.ts`: add a case that `session()` returns events for a run of the task and rejects a run of another task (there is a guard at `tasks-service.ts:158`).
- `workflows/crates/engine/tests/engine.rs`: if not already covered, add a test that session events from a step land in `session_events` in order, that a resume continues the seq, and that a payload over 1 MB is stored with `truncated` and head and tail.
- Checks to run: `ui`: `npm run check && npm run lint && npm test`. `workflows`: `cargo clippy --all-targets && cargo test`. Add `sandbox-container` (`cargo test`) only if it is touched.

## Risks and open questions
- **Main assumption:** the issue wants the existing transcript to be visible and trustworthy on the task page, not new infrastructure. If you see something specific missing when you open a task today (for example, nothing shows for a step, it is not live, or it lacks tool calls and diffs), please say what, and I will target that.
- **Verification is a gap in this plan:** I have only read the code. I have not run the stack, so the "already works" claim, live tailing in particular, is unproven until part 1 is done.
- **Poll vs push:** the live view polls `session_v` every 2 s, though `engine_session` NOTIFY and `realtime/` exist. I would not change it here. Moving to push is a separate change if polling is too slow or heavy.
- **Retention and size:** the default is to keep session events forever, and a diff-heavy step could be large. I add no cap beyond the existing 1 MB per event. A per-Project retention setting would be a follow-up.
- **Older steps:** runs from before ADR-0022 have no session events, and the UI should say so.

## Out of scope
- Object-store transcripts.
- Chat-tab transcripts (`chat-<id>` runs, written by sandbox-proxy).
- Moving the live tail to `realtime/` push.
- Retention settings and pruning.
- Any new table, proto frame or Helm change.

Reply with feedback to revise this plan, or comment /approve to start the implementation.
