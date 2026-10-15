import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import TaskCostTable from './TaskCostTable.svelte';

describe('TaskCostTable', () => {
	it('shows the empty state', async () => {
		render(TaskCostTable, { tasks: [] });
		await expect.element(page.getByText('No usage in this window')).toBeVisible();
	});

	it('shows a dash for a task that reported nothing and $0.00 for a real zero', async () => {
		const screen = await render(TaskCostTable, {
			tasks: [
				{ key: 'a#1', state: 'Done', usage: null },
				{
					key: 'a#2',
					state: 'Done',
					usage: {
						inputTokens: null,
						outputTokens: null,
						cacheReadTokens: null,
						cacheWriteTokens: null,
						models: [],
						costUsd: 0
					}
				}
			]
		});
		const rows = [...screen.container.querySelectorAll('tbody tr')].map((r) => r.textContent ?? '');
		expect(rows[0]).toContain('—');
		expect(rows[0]).not.toContain('$0.00');
		expect(rows[1]).toContain('$0.00');
	});
});
