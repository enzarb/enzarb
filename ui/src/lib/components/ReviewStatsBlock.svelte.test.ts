import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import ReviewStatsBlock from './ReviewStatsBlock.svelte';

describe('ReviewStatsBlock', () => {
	it('shows the rounds and the agent time', async () => {
		const { container } = render(ReviewStatsBlock, {
			stats: { reviewRounds: 3, fixRounds: 2, agentSecs: 3900 }
		});
		await expect.element(page.getByText('Review rounds')).toBeVisible();
		const dd = [...container.querySelectorAll('dd')].map((e) => e.textContent?.trim());
		expect(dd).toEqual(['3', '2', '1 h 5 min']);
	});
});
