import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { JobView } from '#lib/work-items.ts';
import JobTable from './JobTable.svelte';

const job = (id: number, usage: JobView['usage']): JobView => ({
	id,
	kind: id === 1 ? 'agent_triage' : 'agent_plan',
	status: 'succeeded',
	attempt: 1,
	updatedAt: '2026-10-13T12:00:00.000Z',
	outputHtml: null,
	usage
});

describe('JobTable', () => {
	it('titles its empty state one level below the Jobs section heading', async () => {
		render(JobTable, { jobs: [] });
		await expect
			.element(page.getByRole('heading', { name: 'No jobs yet', level: 3 }))
			.toBeVisible();
	});

	it('shows each job its own tokens, cache, model and reported cost', async () => {
		render(JobTable, {
			jobs: [
				job(1, {
					inputTokens: 12_345,
					outputTokens: 1200,
					cacheReadTokens: 9000,
					cacheWriteTokens: 50,
					models: ['claude-sonnet-5-5'],
					costUsd: 0.42
				}),
				job(2, null)
			]
		});
		const first = page.getByRole('row').nth(1);
		await expect.element(first).toHaveTextContent('12.3k');
		await expect.element(first).toHaveTextContent('1.2k');
		await expect.element(first).toHaveTextContent('Cache read 9k · Cache write 50');
		await expect.element(first).toHaveTextContent('claude-sonnet-5-5');
		await expect.element(first).toHaveTextContent('$0.42');
		await expect.element(first).not.toHaveTextContent(/est\./i);
		// a job with no usage reported shows a dash
		const second = page.getByRole('row').nth(2);
		await expect.element(second).not.toHaveTextContent('$');
		await expect.element(second.getByText('—').first()).toBeVisible();
	});

	it('shows a dash for a cost the agent did not report', async () => {
		render(JobTable, {
			jobs: [
				job(1, {
					inputTokens: 5,
					outputTokens: 6,
					cacheReadTokens: null,
					cacheWriteTokens: null,
					models: [],
					costUsd: null
				})
			]
		});
		const row = page.getByRole('row').nth(1);
		await expect.element(row).toHaveTextContent('cost —');
		await expect.element(row).not.toHaveTextContent('$0.00');
	});
});
