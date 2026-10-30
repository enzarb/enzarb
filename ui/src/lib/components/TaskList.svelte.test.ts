import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Task, TaskList as Tasks } from '#lib/tasks.ts';
import TaskList from './TaskList.svelte';

const base = { project: 'p', repo: 'o/r', at: '2026-10-30T12:00:00.000Z', runUrl: null };
const tasks: Task[] = [
	{
		...base,
		kind: 'wait',
		key: 'w',
		number: 1,
		title: 'ask is waiting at w',
		githubUrl: 'https://github.com/o/r/issues/1',
		runUrl: '/projects/p/runs/r1',
		runId: 'r1',
		workflow: 'ask',
		waitingFor: 'issue_comment.created',
		until: null
	},
	{
		...base,
		kind: 'failed',
		key: 'f',
		number: 2,
		title: 'fix failed',
		githubUrl: null,
		runId: 'r2',
		workflow: 'fix',
		reason: 'timeout'
	},
	{
		...base,
		kind: 'triaged',
		key: 't',
		number: 3,
		title: 'Crash',
		githubUrl: 'https://github.com/o/r/issues/3'
	}
];
const props = (list: Tasks | undefined, over = {}) => ({
	list,
	retry: () => {},
	implement: vi.fn(async () => 'Started.'),
	retryRun: vi.fn(async () => 'Restarted.'),
	stop: vi.fn(async () => 'Stopping.'),
	...over
});

describe('TaskList', () => {
	it('shows a skeleton while loading and an empty state', async () => {
		const { unmount } = render(TaskList, props(undefined));
		await expect.element(page.getByRole('status')).toBeVisible();
		unmount();
		render(TaskList, props({ tasks: [], notes: [] }));
		await expect.element(page.getByText('Nothing is waiting for you')).toBeVisible();
	});

	it('groups tasks, links to GitHub and the run, and runs the buttons', async () => {
		const p = props({ tasks, notes: ['GitHub rate limit: o/r is shown from memory'] });
		render(TaskList, p);
		await expect.element(page.getByText('Waiting for you')).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'GitHub' }).first())
			.toHaveAttribute('href', 'https://github.com/o/r/issues/1');
		await expect
			.element(page.getByRole('link', { name: 'Run' }))
			.toHaveAttribute('href', '/projects/p/runs/r1');
		await expect.element(page.getByText('GitHub rate limit')).toBeVisible();
		await page.getByRole('button', { name: 'Implement' }).click();
		expect(p.implement).toHaveBeenCalledWith(tasks[2]);
		await expect.element(page.getByText('Started.')).toBeVisible();
		await page.getByRole('button', { name: 'Retry' }).click();
		expect(p.retryRun).toHaveBeenCalledWith(tasks[1]);
		await page.getByRole('button', { name: 'Stop' }).click();
		expect(p.stop).toHaveBeenCalledWith(tasks[0]);
	});

	it('names the Project on the top-level page', async () => {
		render(TaskList, props({ tasks, notes: [] }, { showProject: true }));
		await expect.element(page.getByText('p · o/r#3')).toBeVisible();
	});
});
