import { describe, expect, it } from 'vitest';
import type { RunDetail } from './runs.ts';
import { splitContext } from './rules-chat.ts';
import { cloneUrl, taskCheckout, taskContextMessage, taskStoreKey } from './task-chat.ts';
import type { Task } from './tasks.ts';

const base = {
	project: 'p',
	repo: 'acme/app',
	number: 12,
	githubUrl: 'https://github.com/acme/app/issues/12',
	runUrl: null,
	at: '2026-10-30T12:00:00Z'
};
const wait: Task = {
	...base,
	kind: 'wait',
	key: 'run:r1',
	title: 'Fix the crash',
	runId: 'r1',
	workflow: 'implement',
	waitingFor: 'issue_comment.created',
	until: null
};
const failed: Task = {
	...base,
	kind: 'failed',
	key: 'run:r2',
	title: 'Fix CI',
	runId: 'r2',
	workflow: 'fix-ci',
	reason: 'timeout'
};
const review: Task = {
	...base,
	kind: 'review',
	openedBy: null,
	key: 'pr:acme/app#50',
	number: 50,
	title: 'Add retries',
	branch: 'ez/implement/r9',
	draft: true
};
const triaged: Task = {
	...base,
	kind: 'triaged',
	key: 'issue:acme/app#12',
	title: 'Crash on start',
	body: 'It crashes.'
};
const run = {
	id: 'r1',
	workflow: 'implement',
	status: 'waiting',
	reason: null,
	nodeRuns: [
		{
			node: 'plan',
			visit: 1,
			status: 'done',
			outputs: { plan: 'Do A then B' },
			error: null,
			git: [{ repo: 'acme/app', branch: 'ez/implement/r1', head_sha: 'abc', pushed: false }]
		},
		{ node: 'build', visit: 1, status: 'failed', outputs: null, error: 'tests failed', git: null }
	]
} as unknown as RunDetail;

describe('taskCheckout', () => {
	it('uses the run branch of a waiting or failed run', () => {
		expect(taskCheckout(wait)).toEqual({ branch: 'ez/implement/r1' });
		expect(taskCheckout(failed)).toEqual({ branch: 'ez/fix-ci/r2' });
		expect(taskCheckout(wait, run)).toEqual({ branch: 'ez/implement/r1' });
	});
	it('uses the head branch of a pull request, starting from it', () => {
		expect(taskCheckout(review)).toEqual({
			branch: 'ez/implement/r9',
			start_ref: 'refs/heads/ez/implement/r9'
		});
	});
	it('starts a new branch for a triaged issue', () => {
		expect(taskCheckout(triaged)).toEqual({ branch: 'enzarb/issue-12', create: true });
	});
});

describe('taskContextMessage', () => {
	it('carries the kind, subject, node results and the push limit', () => {
		const m = taskContextMessage(wait, run);
		expect(m).toContain('Fix the crash');
		expect(m).toContain('"plan":"Do A then B"');
		expect(m).toContain('error: tests failed');
		expect(m).toContain('branch ez/implement/r1');
		expect(m).toContain('You cannot push');
	});
	it('includes the issue text', () => {
		expect(taskContextMessage(triaged)).toContain('It crashes.');
	});
	it('is hidden in the transcript behind the context markers', () => {
		const parts = splitContext(`${taskContextMessage(review)}\n\nhello`);
		expect(parts?.rest).toBe('hello');
		expect(parts?.context).toContain('Add retries');
	});
});

describe('keys', () => {
	it('keeps one conversation per task and clones from the .git URL', () => {
		expect(taskStoreKey(wait)).not.toBe(taskStoreKey(failed));
		expect(cloneUrl('acme/app')).toBe('https://github.com/acme/app.git');
	});
});
