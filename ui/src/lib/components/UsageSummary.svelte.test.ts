import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { sumUsage, type Usage } from '#lib/usage.ts';
import UsageSummary from './UsageSummary.svelte';

const u = (over: Partial<Usage>): Usage => ({
	inputTokens: null,
	outputTokens: null,
	cacheReadTokens: null,
	cacheWriteTokens: null,
	models: [],
	costUsd: null,
	...over
});

describe('UsageSummary', () => {
	it('shows the item total summed over its jobs', async () => {
		const total = sumUsage([
			u({ inputTokens: 10_000, outputTokens: 1000, models: ['m1'], costUsd: 0.25 }),
			u({ inputTokens: 2345, outputTokens: 200, models: ['m1', 'm2'], costUsd: 0.5 })
		]);
		render(UsageSummary, { usage: total, label: 'Total' });
		const line = page.getByTestId('usage');
		await expect.element(line).toHaveTextContent('Total');
		await expect.element(line).toHaveTextContent('in 12.3k');
		await expect.element(line).toHaveTextContent('out 1.2k');
		await expect.element(line).toHaveTextContent('m1, m2');
		await expect.element(line).toHaveTextContent('cost $0.75');
	});

	it('shows a dash when nothing was reported', async () => {
		render(UsageSummary, { usage: null, label: 'Total' });
		await expect.element(page.getByText('Total: —')).toBeVisible();
	});
});
