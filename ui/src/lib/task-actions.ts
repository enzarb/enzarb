import { implementIssue, retryTaskRun, stopTaskRun } from '#lib/tasks.remote.ts';
import type { ActionOutcome, Task } from '#lib/tasks.ts';

/** Anything a command's `.updates(...)` accepts: the queries the page shows. */
type Refreshable = Parameters<ReturnType<typeof implementIssue>['updates']>[number];

type Result = { ok: boolean; message: string; runId?: string | null };
const outcome = (r: Result): ActionOutcome => ({ ok: r.ok, message: r.message, runId: r.runId });

/**
 * The Tasks buttons. Each command refreshes `queries()` on the server in the same round trip, so
 * the list changes without a reload. They resolve to the outcome the page shows.
 */
export function taskActions(queries: () => Refreshable[]) {
	return {
		implement: async (t: Task) =>
			outcome(
				await implementIssue({ name: t.project, repo: t.repo, number: t.number ?? 0 }).updates(
					...queries()
				)
			),
		retryRun: async (t: Task) =>
			outcome(
				t.kind === 'failed'
					? await retryTaskRun({ name: t.project, id: t.runId }).updates(...queries())
					: { ok: false, message: 'Only failed runs can be retried.' }
			),
		stop: async (t: Task) =>
			outcome(
				t.kind === 'wait'
					? await stopTaskRun({ name: t.project, id: t.runId }).updates(...queries())
					: { ok: false, message: 'Only waiting runs can be stopped.' }
			)
	};
}
