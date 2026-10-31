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
		draft: false
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
	implement: vi.fn(async () => 'Started.'),
	retryRun: vi.fn(async () => 'Restarted.'),
	stop: vi.fn(async () => 'Stopping.'),
	...over
});

describe('defaultAction', () => {
	it('is Implement, Retry, Stop for now and Open diff by kind', () => {
		expect(tasks.map((t) => defaultAction(t).label)).toEqual([
			'Stop for now',
			'Retry',
			'Open diff',
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

	it('shows a pull request with its branch and an Open diff link', async () => {
		render(TaskList, props({ tasks, notes: [] }, { selected: 'pr:o/r#4' }));
		await expect.element(page.getByText('ez/fix/abc')).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'Open diff' }))
			.toHaveAttribute('href', 'https://github.com/o/r/pull/4/files');
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
