import type { ChatCheckout } from '@enzarb/proto';
import type { RunDetail } from './runs.ts';
import { CONTEXT_END, CONTEXT_START } from './rules-chat.ts';
import { KIND_NAMES, type Task } from './tasks.ts';

/** The URL the sandbox cloned the task's repository from (the key of `open.repo`). */
export const cloneUrl = (repo: string) => `https://github.com/${repo}.git`;

/** Branch of a new chat on a triaged issue: a fresh branch from the default branch. */
export const issueBranch = (number: number | null) => `enzarb/issue-${number ?? 'new'}`;

/**
 * The branch a task's chat works on (`open.checkout`):
 * - a waiting or failed run: the run's branch `ez/<workflow>/<run>` (the node that pushed or
 *   checked it out names it; else the convention); the sandbox starts it from the run's own
 *   checkout, else `origin`, else the default branch;
 * - a pull request: its head branch (these are `ez/*` branches of the same repository);
 * - a triaged issue: a new branch from the default branch.
 */
export function taskCheckout(task: Task, run?: RunDetail | null): ChatCheckout {
	switch (task.kind) {
		case 'wait':
		case 'failed': {
			const named = run?.nodeRuns.flatMap((n) => n.git ?? []).find((g) => g.repo === task.repo);
			return { branch: named?.branch || `ez/${task.workflow}/${task.runId}` };
		}
		case 'review':
			return { branch: task.branch, start_ref: `refs/heads/${task.branch}` };
		case 'triaged':
			return { branch: issueBranch(task.number) };
	}
}

/** Key the chat's ACP session id is kept under: one conversation per task. */
export const taskStoreKey = (task: Task) => `${task.project}.task.${task.key}`;

const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, n)}…` : s);

/** What the node results (plan, failure reason) tell the agent about a run. */
function runLines(run: RunDetail): string[] {
	const out: string[] = [`Run ${run.id} of workflow ${run.workflow}: ${run.status}.`];
	if (run.reason) out.push(`Reason: ${run.reason}`);
	for (const n of run.nodeRuns) {
		out.push(
			`- node ${n.node} (visit ${n.visit}): ${n.status}${n.error ? `, error: ${clip(n.error, 600)}` : ''}`
		);
		if (n.outputs && Object.keys(n.outputs).length > 0) {
			out.push(`  result: ${clip(JSON.stringify(n.outputs), 3000)}`);
		}
	}
	return out;
}

/** What the user may ask for, by kind. */
function offers(task: Task): string {
	switch (task.kind) {
		case 'wait':
			return 'The run is waiting for the user. They may ask you to explain the plan, change it, or answer the question it waits on.';
		case 'failed':
			return 'The run failed. They may ask you to find why, to fix the code on this branch, or to explain what to change before a retry.';
		case 'review':
			return 'The user is reviewing this pull request. They may ask you to explain it, review it, or fix something on its branch.';
		case 'triaged':
			return 'The user may ask you to investigate the issue, plan a fix, or start a fix on a new branch.';
	}
}

/**
 * The first message of a task chat: the kind, the subject's title and text, what the run did,
 * the working branch and the ground rules. The transcript shows it collapsed.
 */
export function taskContextMessage(task: Task, run?: RunDetail | null): string {
	const co = taskCheckout(task, run);
	const lines = [
		CONTEXT_START,
		`You are helping the user with one task in ${task.repo}: ${KIND_NAMES[task.kind]}.`,
		`Title: ${task.title}`
	];
	if (task.number)
		lines.push(`Subject: #${task.number}${task.githubUrl ? ` (${task.githubUrl})` : ''}`);
	if (task.kind === 'triaged' && task.body) lines.push('', 'Issue text:', clip(task.body, 4000));
	if (task.kind === 'review')
		lines.push(`Pull request branch: ${task.branch}${task.draft ? ' (draft)' : ''}`);
	if ((task.kind === 'wait' || task.kind === 'failed') && run) lines.push('', ...runLines(run));
	if (task.kind === 'wait') lines.push(`Waiting for: ${task.waitingFor}`);
	lines.push(
		'',
		offers(task),
		`Your working directory is a checkout of ${task.repo} on branch ${co.branch}.`,
		'You can read and edit files and commit on this branch. You cannot push: your commits stay in this sandbox until a run pushes them, so tell the user when a change needs a run (for a pull request, ask them to start one that pushes).',
		CONTEXT_END
	);
	return lines.join('\n');
}
