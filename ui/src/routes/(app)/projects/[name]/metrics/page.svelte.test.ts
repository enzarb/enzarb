import { page as browser } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const h = vi.hoisted(() => ({
	search: '',
	data: undefined as unknown,
	goto: vi.fn(),
	args: vi.fn()
}));

vi.mock('$app/state', () => ({
	page: {
		get url() {
			return new URL(`http://localhost/projects/alpha/metrics${h.search}`);
		},
		params: { name: 'alpha' }
	}
}));
vi.mock('$app/navigation', () => ({ goto: h.goto }));
vi.mock('#lib/metrics.remote.ts', () => ({
	getProjectUsage: (a: unknown) => {
		h.args(a);
		return { current: h.data, error: undefined };
	}
}));

import Metrics from './+page.svelte';

const none = { reporters: 0, avg: null, median: null };
const empty = {
	total: { items: 0, usage: null, input: none, output: none, cost: none },
	tasks: []
};

beforeEach(() => {
	h.search = '';
	h.data = empty;
	h.goto.mockClear();
	h.args.mockClear();
});

describe('metrics tab', () => {
	it('shows the empty state for a window without usage', async () => {
		render(Metrics);
		await expect.element(browser.getByText('No usage in this window')).toBeVisible();
		expect(h.args).toHaveBeenCalledWith({ name: 'alpha', window: '7d' });
	});

	it('reads the window from the URL', async () => {
		h.search = '?from=2026-01-01&to=2026-01-31';
		render(Metrics);
		await expect.element(browser.getByText('No usage in this window')).toBeVisible();
		expect(h.args).toHaveBeenCalledWith({ name: 'alpha', from: '2026-01-01', to: '2026-01-31' });
	});

	it('falls back to 7d with a notice for an invalid window', async () => {
		h.search = '?window=5y';
		render(Metrics);
		await expect.element(browser.getByText('Showing the last 7 days')).toBeVisible();
		expect(h.args).toHaveBeenCalledWith({ name: 'alpha', window: '7d' });
	});

	it('puts the chosen window in the URL', async () => {
		render(Metrics);
		await browser.getByRole('button', { name: '24h' }).click();
		expect(h.goto).toHaveBeenCalledWith(
			'?window=24h',
			expect.objectContaining({ replace: true, reset: false })
		);
	});

	it('shows totals, the chart and the table for a window with usage', async () => {
		h.data = {
			total: {
				items: 2,
				usage: {
					inputTokens: 1000,
					outputTokens: 10,
					cacheReadTokens: null,
					cacheWriteTokens: null,
					models: [],
					costUsd: 0.5
				},
				input: none,
				output: none,
				cost: { reporters: 1, avg: 0.5, median: 0.5 }
			},
			tasks: [
				{
					key: 'a#1',
					state: 'Done',
					usage: {
						inputTokens: null,
						outputTokens: null,
						cacheReadTokens: null,
						cacheWriteTokens: null,
						models: [],
						costUsd: 0.5
					}
				},
				{ key: 'a#2', state: 'Done', usage: null }
			]
		};
		const screen = render(Metrics);
		await expect.element(browser.getByRole('table', { name: 'Cost per task' })).toBeVisible();
		expect(screen.container.querySelectorAll('[data-testid="bar"]').length).toBe(1);
		await expect.element(browser.getByTestId('usage-summary')).toBeVisible();
	});
});
