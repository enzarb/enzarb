import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { defaultAction, type Task, type TaskList as Tasks } from '#lib/tasks.ts';
import TaskList from './TaskList.svelte';

const base = { project: 'p', repo: 'o/r', at: '2026-10-30T12:00:00.000Z', runUrl: null };
const tasks: Task[] = [
	{
		...base,
		kind: 'wait',
		key: 'run:r1',
		number: 1,
		title: 'ask is waiting at w',
		githubUrl: 'https://github.com/o/r/issues/1',
		runUrl: '/projects/p/runs/r1',
		runId: 'r1aaaaaaaa',
		workflow: 'ask',
		waitingFor: 'issue_comment.created',
		until: null
	},
	{
		...base,
		kind: 'failed',
		key: 'run:r2',
		number: 2,
		title: 'fix failed',
		githubUrl: null,
		runId: 'r2',
		workflow: 'fix',
		reason: 'timeout'
	},
	{
		...base,
		kind: 'review',
		key: 'pr:o/r#4',
		number: 4,
		title: 'Fix crash',
		githubUrl: 'https://github.com/o/r/pull/4',
		branch: 'ez/fix/abc',
		draft: false,
		openedBy: { id: '1a2b3c4d5e6f', workflow: 'implement-labeled-issues' },
		runUrl: '/projects/p/runs/1a2b3c4d5e6f'
	},
	{
		...base,
		kind: 'triaged',
		key: 'issue:o/r#3',
		number: 3,
		title: 'Crash',
		body: 'It crashes on start.',
		githubUrl: 'https://github.com/o/r/issues/3'
	}
];
const props = (list: Tasks | undefined, over = {}) => ({
	list,
	href: (key: string) => `/tasks?task=${encodeURIComponent(key)}`,
	backHref: '/tasks',
	retry: () => {},
	implement: vi.fn(async () => ({ ok: true, message: 'Started.', runId: 'abcdef1234567' })),
	retryRun: vi.fn(async () => ({ ok: true, message: 'Restarted.', runId: null })),
	stop: vi.fn(async () => ({ ok: true, message: 'Stopping.', runId: null })),
	answerPlan: vi.fn(async () => ({
		ok: true,
		message: 'Plan approved: implementation started.',
		runId: 'r1aaaaaaaa'
	})),
	mergePull: vi.fn(async () => ({ ok: true, message: 'Approved and merged #4.', steps: [] })),
	...over
});

describe('defaultAction', () => {
	it('is Implement, Retry, Stop for now and Approve & merge by kind', () => {
		expect(tasks.map((t) => defaultAction(t).label)).toEqual([
			'Stop for now',
			'Retry',
			'Approve & merge',
			'Implement'
		]);
	});
});

describe('TaskList', () => {
	it('shows a skeleton while loading and an empty state', async () => {
		const { unmount } = render(TaskList, props(undefined));
		await expect.element(page.getByRole('status')).toBeVisible();
		unmount();
		render(TaskList, props({ tasks: [], notes: [] }));
		await expect.element(page.getByText('Nothing is waiting for you')).toBeVisible();
	});

	it('groups tasks in the sidebar; entries select through the URL and have no buttons', async () => {
		render(TaskList, props({ tasks, notes: ['GitHub rate limit: o/r is shown from memory'] }));
		await expect.element(page.getByText('Waiting for you')).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: /Crash/ }))
			.toHaveAttribute('href', '/tasks?task=issue%3Ao%2Fr%233');
		await expect.element(page.getByRole('button', { name: 'Implement' })).not.toBeInTheDocument();
		await expect.element(page.getByText('GitHub rate limit')).toBeVisible();
		await expect.element(page.getByText('Select a task to see it here.')).toBeInTheDocument();
	});

	it('shows the selected issue with a readable link and runs its default action', async () => {
		const p = props({ tasks, notes: [] }, { selected: 'issue:o/r#3' });
		render(TaskList, p);
		await expect.element(page.getByText('It crashes on start.')).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'r#3', exact: true }))
			.toHaveAttribute('href', 'https://github.com/o/r/issues/3');
		await expect.element(page.getByRole('link', { name: 'GitHub' })).not.toBeInTheDocument();
		await page.getByRole('button', { name: 'Implement' }).click();
		expect(p.implement).toHaveBeenCalledWith(tasks[3]);
		await expect.element(page.getByText('Started.')).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'implement-labeled-issues #abcdef12' }))
			.toHaveAttribute('href', '/projects/p/runs/abcdef1234567');
	});

	it('keeps the confirmation when the task leaves the list, until another is selected', async () => {
		const p = props({ tasks, notes: [] }, { selected: 'issue:o/r#3' });
		const view = render(TaskList, p);
		await page.getByRole('button', { name: 'Implement' }).click();
		await view.rerender({ ...p, list: { tasks: tasks.slice(0, 3), notes: [] } });
		await expect.element(page.getByTestId('task-done')).toHaveTextContent('Started.');
		await expect.element(page.getByText('Select a task to see it here.')).not.toBeInTheDocument();
		await view.rerender({
			...p,
			list: { tasks: tasks.slice(0, 3), notes: [] },
			selected: 'pr:o/r#4'
		});
		await expect.element(page.getByTestId('task-done')).not.toBeInTheDocument();
	});

	it('shows an error outcome as a plain message, not a confirmation', async () => {
		const p = props(
			{ tasks, notes: [] },
			{
				selected: 'issue:o/r#3',
				implement: vi.fn(async () => ({ ok: false, message: 'Need write access.' }))
			}
		);
		render(TaskList, p);
		await page.getByRole('button', { name: 'Implement' }).click();
		await expect.element(page.getByText('Need write access.')).toBeVisible();
		await expect.element(page.getByTestId('task-done')).not.toBeInTheDocument();
	});

	it('marks a cut issue body and links to the rest', async () => {
		const cut = tasks.map((t) =>
			t.kind === 'triaged' ? { ...t, body: 'Long text', bodyTruncated: true } : t
		);
		render(TaskList, props({ tasks: cut, notes: [] }, { selected: 'issue:o/r#3' }));
		await expect.element(page.getByText('Long text…')).toBeVisible();
		await expect.element(page.getByText(/Read the rest on/)).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'r#3', exact: true }).nth(1))
			.toHaveAttribute('href', 'https://github.com/o/r/issues/3');
	});

	it('does not mark an uncut body', async () => {
		render(TaskList, props({ tasks, notes: [] }, { selected: 'issue:o/r#3' }));
		await expect.element(page.getByText('It crashes on start.')).toBeVisible();
		await expect.element(page.getByText(/Read the rest on/)).not.toBeInTheDocument();
	});

	it('Retry for a failed run, Stop for now for a waiting run', async () => {
		const failed = props({ tasks, notes: [] }, { selected: 'run:r2', run: null });
		const { unmount } = render(TaskList, failed);
		await page.getByRole('button', { name: 'Retry' }).click();
		expect(failed.retryRun).toHaveBeenCalledWith(tasks[1]);
		unmount();
		const wait = props({ tasks, notes: [] }, { selected: 'run:r1', run: null });
		render(TaskList, wait);
		await expect
			.element(page.getByRole('link', { name: 'ask #r1aaaaaa' }))
			.toHaveAttribute('href', '/projects/p/runs/r1');
		await page.getByRole('button', { name: 'Stop for now' }).click();
		expect(wait.stop).toHaveBeenCalledWith(tasks[0]);
	});

	const files = [
		{
			filename: 'src/a.ts',
			status: 'modified',
			additions: 1,
			deletions: 1,
			patch: '@@ -1,2 +1,2 @@\n keep\n-old\n+new'
		},
		{ filename: 'logo.png', status: 'added', additions: 0, deletions: 0 },
		{ filename: 'big.ts', status: 'added', additions: 5000, deletions: 0 }
	];
	const review = {
		pull: null,
		files,
		filesTruncated: false,
		checks: [
			{ name: 'build', status: 'completed', conclusion: 'success', url: '' },
			{ name: 'lint', status: 'completed', conclusion: 'failure', url: '' },
			{ name: 'e2e', status: 'in_progress', conclusion: null, url: '' }
		],
		notes: {
			html: '<p><strong>Looks good</strong></p>',
			run: { id: '8888888888', workflow: 'review-pull-requests' },
			source: 'run' as const
		},
		problems: []
	};

	it('shows a pull request: notes with run link, checks, inline diff, and the run that opened it', async () => {
		render(TaskList, props({ tasks, notes: [] }, { selected: 'pr:o/r#4', review }));
		await expect.element(page.getByText('ez/fix/abc')).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'implement-labeled-issues #1a2b3c4d' }))
			.toHaveAttribute('href', '/projects/p/runs/1a2b3c4d5e6f');
		await expect.element(page.getByTestId('review-notes')).toHaveTextContent('Looks good');
		await expect
			.element(page.getByRole('link', { name: 'review-pull-requests #88888888' }))
			.toBeVisible();
		await expect
			.element(page.getByTestId('checks-summary'))
			.toHaveTextContent('1 passed, 1 failed (lint), 1 pending');
		await expect.element(page.getByText('src/a.ts')).toBeVisible();
		const added = document.querySelector('.ez-diff__line--add');
		const removed = document.querySelector('.ez-diff__line--del');
		expect(added?.textContent).toContain('new');
		expect(removed?.textContent).toContain('old');
		expect(document.querySelectorAll('.ez-diff__line--add .ez-diff__no')[1]?.textContent).toBe('2');
		expect(document.body.textContent).toContain('Too large to show inline');
		await expect.element(page.getByRole('link', { name: 'Open diff' })).not.toBeInTheDocument();
	});

	it('Approve & merge runs the merge and shows the steps', async () => {
		const p = props(
			{ tasks, notes: [] },
			{
				selected: 'pr:o/r#4',
				review,
				mergePull: vi.fn(async () => ({
					ok: false,
					message: 'Approved, not merged. Checks are failing: lint.',
					steps: [
						{ id: 'access', label: 'Check your write access', result: 'done' },
						{ id: 'approve', label: 'Approve the pull request', result: 'done' },
						{
							id: 'status',
							label: 'Check mergeability and checks',
							result: 'failed',
							message: 'Approved, not merged. Checks are failing: lint.'
						},
						{ id: 'merge', label: 'Squash and merge', result: 'skipped' }
					]
				}))
			}
		);
		render(TaskList, p);
		await page.getByRole('button', { name: 'Approve & merge' }).click();
		expect(p.mergePull).toHaveBeenCalledWith(tasks[2]);
		await expect
			.element(page.getByRole('list', { name: 'Approve and merge progress' }))
			.toBeVisible();
		await expect.element(page.getByText('Checks are failing: lint.').first()).toBeVisible();
	});

	const planTask = tasks[0]!;
	it('shows the plan of a waiting run; Approve and Revise answer it', async () => {
		const p = props(
			{ tasks, notes: [] },
			{ selected: 'run:r1', plan: { html: '<h2>Plan</h2><p>Do the thing</p>' }, run: null }
		);
		render(TaskList, p);
		await expect.element(page.getByTestId('plan-body')).toHaveTextContent('Do the thing');
		await page.getByRole('button', { name: 'Revise' }).click();
		await page.getByLabelText('What should change in the plan?').fill('also add docs');
		await page.getByRole('button', { name: 'Send revision' }).click();
		expect(p.answerPlan).toHaveBeenCalledWith(planTask, 'revise', 'also add docs');
		await page.getByRole('button', { name: 'Approve', exact: true }).click();
		expect(p.answerPlan).toHaveBeenCalledWith(planTask, 'approve', undefined);
		await expect
			.element(page.getByTestId('task-done'))
			.toHaveTextContent('Plan approved: implementation started.');
	});

	it('shows a failed answer without confirming it', async () => {
		const p = props(
			{ tasks, notes: [] },
			{
				selected: 'run:r1',
				plan: { html: '<p>x</p>' },
				run: null,
				answerPlan: vi.fn(async () => ({ ok: false, message: 'The run is no longer waiting.' }))
			}
		);
		render(TaskList, p);
		await page.getByRole('button', { name: 'Approve', exact: true }).click();
		await expect.element(page.getByRole('alert')).toHaveTextContent('no longer waiting');
	});

	it('keeps Stop for a wait without a plan', async () => {
		render(TaskList, props({ tasks, notes: [] }, { selected: 'run:r1', run: null }));
		await expect.element(page.getByRole('button', { name: 'Stop for now' })).toBeVisible();
		await expect
			.element(page.getByRole('button', { name: 'Approve', exact: true }))
			.not.toBeInTheDocument();
	});

	it('has a back control and a disabled chat bar in the detail', async () => {
		render(TaskList, props({ tasks, notes: [] }, { selected: 'pr:o/r#4' }));
		await expect
			.element(page.getByRole('link', { name: 'All tasks' }))
			.toHaveAttribute('href', '/tasks');
		await expect
			.element(page.getByPlaceholder('Chat with the agent about this task: coming next'))
			.toBeDisabled();
	});

	it('ignores a stale selection', async () => {
		render(TaskList, props({ tasks, notes: [] }, { selected: 'issue:gone' }));
		await expect.element(page.getByText('Select a task to see it here.')).toBeInTheDocument();
	});

	it('names the Project on the top-level page', async () => {
		render(TaskList, props({ tasks, notes: [] }, { showProject: true }));
		await expect.element(page.getByText(/Issue r#3 · p · triaged/)).toBeVisible();
	});
});
