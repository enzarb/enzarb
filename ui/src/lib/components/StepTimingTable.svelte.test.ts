import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { StepTiming } from '#lib/server/metrics-service.ts';
import StepTimingTable from './StepTimingTable.svelte';

const timing: StepTiming = {
	states: [
		{
			name: 'Implementing',
			active: { n: 4, min: 5, max: 200, mean: 60, median: 50, p95: 100 },
			openExcluded: 1,
			waiting: null
		},
		{
			name: 'Reviewing',
			active: { n: 2, min: 1, max: 40, mean: 20, median: 25, p95: null },
			openExcluded: 0,
			waiting: null
		},
		{ name: 'Blocked', active: null, openExcluded: 0, waiting: { count: 2, totalS: 7200 } }
	],
	jobs: [{ kind: 'agent', stats: { n: 3, min: 10, max: 30, mean: 20, median: 20, p95: 29 } }]
};

describe('StepTimingTable', () => {
	it('shows the empty state when nothing is in the window', async () => {
		render(StepTimingTable, { timing: { states: [], jobs: [] } });
		await expect.element(page.getByText('No step timing in this window')).toBeVisible();
	});

	it('shows all six statistics per active state, with dashes for missing ones', async () => {
		const screen = await render(StepTimingTable, { timing });
		const table = screen.container.querySelector('table[aria-label="Active time per state"]')!;
		const cells = (i: number) =>
			[...table.querySelectorAll('tbody tr')[i].children].map((c) => c.textContent?.trim());
		expect(cells(0).slice(0, 7)).toEqual([
			'Implementing',
			'4',
			'5s',
			'3m 20s',
			'1m 00s',
			'50s',
			'1m 40s'
		]);
		expect(cells(1)[6]).toBe('–');
	});

	it('keeps waiting in its own table, not in the active one', async () => {
		const screen = await render(StepTimingTable, { timing });
		const active = screen.container.querySelector('table[aria-label="Active time per state"]')!;
		const waiting = screen.container.querySelector('table[aria-label="Waiting time per state"]')!;
		expect(active.textContent).not.toContain('Blocked');
		expect(waiting.textContent).toContain('Blocked');
		expect(waiting.textContent).toContain('2h 00m');
		expect(waiting.textContent).not.toContain('Implementing');
	});

	it('scales bars to the slowest value in the table', async () => {
		const screen = await render(StepTimingTable, { timing });
		const active = screen.container.querySelector('table[aria-label="Active time per state"]')!;
		const pcts = [...active.querySelectorAll<HTMLElement>('[data-testid="bar"]')].map((b) =>
			Number(b.style.getPropertyValue('--pct'))
		);
		// Largest of p95 and max is 200s.
		expect(pcts).toEqual([25, 12.5]);
		const marks = active.querySelectorAll<HTMLElement>('[data-testid="p95"]');
		expect(marks).toHaveLength(1);
		expect(Number(marks[0].style.getPropertyValue('--pct'))).toBe(50);
	});

	it('lists jobs by kind in a secondary table', async () => {
		await render(StepTimingTable, { timing });
		await expect.element(page.getByRole('table', { name: 'Job duration by kind' })).toBeVisible();
		await expect.element(page.getByRole('rowheader', { name: 'agent' })).toBeVisible();
	});

	it('notes visits that are still open', async () => {
		await render(StepTimingTable, { timing });
		await expect.element(page.getByText('1 visit still open, not counted.')).toBeVisible();
	});
});
