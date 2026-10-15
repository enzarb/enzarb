import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { formatBytes } from '#lib/metrics-format.ts';
import SeriesChart from './SeriesChart.svelte';

const base = { title: 'Memory', format: formatBytes, from: 0, to: 600, stepMs: 10 };
const lines = (v: (number | null)[], step = 10) => [
	{ label: 'Average', points: v.map((x, i) => ({ t: i * step, v: x })) }
];

const count = (c: HTMLElement, id: string) => c.querySelectorAll(`[data-testid="${id}"]`).length;

describe('SeriesChart', () => {
	it('draws one line and one area for a continuous series, labelled for screen readers', async () => {
		const screen = await render(SeriesChart, { ...base, lines: lines([1, 2, 3]) });
		expect(count(screen.container, 'line')).toBe(1);
		expect(count(screen.container, 'area')).toBe(1);
		await expect.element(page.getByRole('img', { name: 'Memory' })).toBeInTheDocument();
	});

	it('splits on a not-reported value and on a missing bucket', async () => {
		const screen = await render(SeriesChart, {
			...base,
			lines: [
				{
					label: 'Average',
					points: [
						{ t: 0, v: 1 },
						{ t: 10, v: 2 },
						{ t: 20, v: null },
						{ t: 30, v: 3 },
						{ t: 40, v: 4 },
						{ t: 100, v: 5 },
						{ t: 110, v: 6 }
					]
				}
			]
		});
		expect(count(screen.container, 'line')).toBe(3);
		const text = screen.container.querySelector('table')!.textContent;
		expect(text).toContain('not reported');
	});

	it('draws a lone point as a dot, not a line', async () => {
		const screen = await render(SeriesChart, { ...base, lines: lines([5]) });
		expect(count(screen.container, 'dot')).toBe(1);
		expect(count(screen.container, 'line')).toBe(0);
	});

	it('draws nothing for an empty series', async () => {
		const screen = await render(SeriesChart, { ...base, lines: lines([]) });
		expect(count(screen.container, 'line')).toBe(0);
		expect(count(screen.container, 'dot')).toBe(0);
	});

	it('shows a legend only for several lines', async () => {
		const two = [
			...lines([1, 2]),
			{
				label: 'Peak',
				points: [
					{ t: 0, v: 2 },
					{ t: 10, v: 3 }
				]
			}
		];
		const screen = await render(SeriesChart, { ...base, lines: two });
		expect(screen.container.querySelector('.ez-serieschart__legend')).not.toBeNull();
		expect(count(screen.container, 'area')).toBe(1);
	});
});
