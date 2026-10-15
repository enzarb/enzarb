import { page as browser } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const h = vi.hoisted(() => ({
	search: '',
	data: undefined as unknown,
	error: undefined as unknown,
	args: vi.fn(),
	timing: vi.fn(),
	sandbox: vi.fn()
}));

vi.mock('$app/state', () => ({
	page: {
		get url() {
			return new URL(`http://localhost/projects/alpha/metrics${h.search}`);
		},
		params: { name: 'alpha' }
	}
}));
vi.mock('#lib/metrics.remote.ts', () => ({
	getProjectUsage: (a: unknown) => {
		h.args(a);
		return { current: h.data, error: h.error };
	},
	getProjectStepTiming: (a: unknown) => {
		h.timing(a);
		return { current: undefined, error: undefined };
	},
	getSandboxStats: (a: unknown) => {
		h.sandbox(a);
		return { current: undefined, error: undefined };
	}
}));

import Usage from './+page.svelte';

const none = { reporters: 0, avg: null, median: null };
const empty = {
	total: { items: 0, usage: null, input: none, output: none, cost: none },
	tasks: []
};

beforeEach(() => {
	h.search = '';
	h.data = empty;
	h.error = undefined;
	h.args.mockClear();
	h.timing.mockClear();
	h.sandbox.mockClear();
});

describe('usage subpage', () => {
	it('shows a skeleton while loading', async () => {
		h.data = undefined;
		render(Usage);
		await expect.element(browser.getByText('Loading usage')).toBeInTheDocument();
	});

	it('shows an error when loading fails', async () => {
		h.data = undefined;
		h.error = new Error('boom');
		render(Usage);
		await expect.element(browser.getByText('Could not load usage')).toBeVisible();
	});

	it('shows the empty state for a window without usage', async () => {
		render(Usage);
		await expect.element(browser.getByText('No usage in this window')).toBeVisible();
		expect(h.args).toHaveBeenCalledWith({ name: 'alpha', window: '7d' });
	});

	it('fetches only usage', async () => {
		render(Usage);
		await expect.element(browser.getByText('No usage in this window')).toBeVisible();
		expect(h.timing).not.toHaveBeenCalled();
		expect(h.sandbox).not.toHaveBeenCalled();
	});

	it('reads the window from the URL', async () => {
		h.search = '?from=2026-01-01&to=2026-01-31';
		render(Usage);
		await expect.element(browser.getByText('No usage in this window')).toBeVisible();
		expect(h.args).toHaveBeenCalledWith({ name: 'alpha', from: '2026-01-01', to: '2026-01-31' });
	});

	it('falls back to 7d for an invalid window', async () => {
		h.search = '?window=5y';
		render(Usage);
		await expect.element(browser.getByText('No usage in this window')).toBeVisible();
		expect(h.args).toHaveBeenCalledWith({ name: 'alpha', window: '7d' });
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
		const screen = render(Usage);
		await expect
			.element(browser.getByRole('table', { name: 'Cost per task', exact: true }))
			.toBeVisible();
		expect(screen.container.querySelectorAll('[data-testid="bar"]').length).toBe(1);
		await expect.element(browser.getByTestId('usage-summary')).toBeVisible();
	});
});
