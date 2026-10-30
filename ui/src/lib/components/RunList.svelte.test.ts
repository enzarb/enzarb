import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { RunSummary } from '#lib/runs.ts';
import RunList from './RunList.svelte';

const r = (over: Partial<RunSummary> = {}): RunSummary => ({
	id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
	workflow: 'fix',
	project: 'p',
	repo: 'o/r',
	subject: { kind: 'issue', number: 3 },
	eventId: 1,
	sha: 'abc',
	status: 'done',
	test: false,
	currentNode: null,
	reason: null,
	stopRequested: false,
	createdAt: '2026-10-30T12:00:00.000Z',
	updatedAt: '2026-10-30T12:00:00.000Z',
	finishedAt: null,
	...over
});
const base = { base: '/projects/p/runs', retry: () => {} };

describe('RunList', () => {
	it('shows a skeleton while loading', async () => {
		render(RunList, { ...base, runs: undefined });
		await expect.element(page.getByRole('status')).toBeVisible();
	});

	it('shows an empty state', async () => {
		render(RunList, { ...base, runs: [] });
		await expect.element(page.getByText('No runs yet')).toBeVisible();
	});

	it('shows an error with retry', async () => {
		render(RunList, { ...base, runs: undefined, failed: true });
		await expect.element(page.getByText('Could not load runs')).toBeVisible();
	});

	it('links each run to its detail with status and test marker', async () => {
		render(RunList, {
			...base,
			runs: [
				r(),
				r({
					id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
					test: true,
					status: 'failed',
					reason: 'timeout'
				})
			]
		});
		await expect
			.element(page.getByRole('link', { name: /fix.*done/ }))
			.toHaveAttribute('href', '/projects/p/runs/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa');
		await expect.element(page.getByText('Test')).toBeVisible();
		await expect.element(page.getByText('timeout')).toBeVisible();
	});
});
