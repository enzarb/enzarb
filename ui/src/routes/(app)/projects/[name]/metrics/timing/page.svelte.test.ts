import { page as browser } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const h = vi.hoisted(() => ({
	data: undefined as unknown,
	error: undefined as unknown,
	usage: vi.fn(),
	args: vi.fn()
}));

vi.mock('$app/state', () => ({
	page: {
		get url() {
			return new URL('http://localhost/projects/alpha/metrics/timing?window=24h');
		},
		params: { name: 'alpha' }
	}
}));
vi.mock('#lib/metrics.remote.ts', () => ({
	getProjectUsage: (a: unknown) => {
		h.usage(a);
		return { current: undefined, error: undefined };
	},
	getProjectStepTiming: (a: unknown) => {
		h.args(a);
		return { current: h.data, error: h.error };
	}
}));

import Timing from './+page.svelte';

beforeEach(() => {
	h.data = undefined;
	h.error = undefined;
	h.usage.mockClear();
	h.args.mockClear();
});

describe('step timing subpage', () => {
	it('shows a skeleton while loading', async () => {
		render(Timing);
		await expect.element(browser.getByText('Loading step timing')).toBeInTheDocument();
	});

	it('shows an error when loading fails', async () => {
		h.error = new Error('boom');
		render(Timing);
		await expect.element(browser.getByText('Could not load step timing')).toBeVisible();
	});

	it('shows the table and fetches only step timing', async () => {
		h.data = {
			states: [
				{
					name: 'Implementing',
					active: { n: 2, min: 1, max: 9, mean: 5, median: 5, p95: 9 },
					openExcluded: 0,
					waiting: null
				}
			],
			jobs: []
		};
		render(Timing);
		await expect.element(browser.getByTestId('step-timing')).toBeVisible();
		expect(h.args).toHaveBeenCalledWith({ name: 'alpha', window: '24h' });
		expect(h.usage).not.toHaveBeenCalled();
	});

	it('shows the empty state for a window without timing', async () => {
		h.data = { states: [], jobs: [] };
		render(Timing);
		await expect.element(browser.getByText('No step timing in this window')).toBeVisible();
	});
});
