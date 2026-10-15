import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { formatCost } from '#lib/usage.ts';
import BarChart from './BarChart.svelte';

describe('BarChart', () => {
	it('draws a bar per reported value and none for not-reported ones', async () => {
		const screen = await render(BarChart, {
			title: 'Cost per task',
			format: formatCost,
			bars: [
				{ label: 'a#1', value: 2 },
				{ label: 'a#2', value: null },
				{ label: 'a#3', value: 0 }
			]
		});
		expect(screen.container.querySelectorAll('[data-testid="bar"]').length).toBe(2);
		await expect.element(page.getByRole('img', { name: 'Cost per task' })).toBeInTheDocument();
		const text = screen.container.querySelector('table')!.textContent;
		expect(text).toContain('a#2—');
		expect(text).toContain('a#3$0.00');
	});

	it('renders no bars for an empty list', async () => {
		const screen = await render(BarChart, { title: 'x', format: formatCost, bars: [] });
		expect(screen.container.querySelectorAll('[data-testid="bar"]').length).toBe(0);
	});
});
