import { implementIssue, retryTaskRun, stopTaskRun } from '#lib/tasks.remote.ts';
import type { Task } from '#lib/tasks.ts';

/** Anything a command's `.updates(...)` accepts: the queries the page shows. */
type Refreshable = Parameters<ReturnType<typeof implementIssue>['updates']>[number];

/**
 * The Tasks buttons. Each command refreshes `queries()` on the server in the same round trip, so
 * the list changes without a reload. They resolve to the sentence the page shows.
 */
export function taskActions(queries: () => Refreshable[]) {
	const text = (r: { ok: boolean; message: string }) => r.message;
	return {
		implement: async (t: Task) =>
			text(
				await implementIssue({ name: t.project, repo: t.repo, number: t.number ?? 0 }).updates(
					...queries()
				)
			),
		retryRun: async (t: Task) =>
			text(
				t.kind === 'failed'
					? await retryTaskRun({ name: t.project, id: t.runId }).updates(...queries())
					: { ok: false, message: 'Only failed runs can be retried.' }
			),
		stop: async (t: Task) =>
			text(
				t.kind === 'wait'
					? await stopTaskRun({ name: t.project, id: t.runId }).updates(...queries())
					: { ok: false, message: 'Only waiting runs can be stopped.' }
			)
	};
}
