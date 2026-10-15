import { page as browser } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const h = vi.hoisted(() => ({
	path: '/projects/alpha/metrics',
	search: '',
	goto: vi.fn()
}));

vi.mock('$app/state', () => ({
	page: {
		get url() {
			return new URL(`http://localhost${h.path}${h.search}`);
		},
		params: { name: 'alpha' }
	}
}));
vi.mock('$app/navigation', () => ({ goto: h.goto }));

import Layout from './+layout.svelte';
import LayoutHost from '#lib/components/__fixtures__/LayoutHost.svelte';
import Empty from '#lib/components/__fixtures__/TerminalStub.svelte';

const mount = () => render(LayoutHost, { Layout, Child: Empty } as never);

beforeEach(() => {
	h.path = '/projects/alpha/metrics';
	h.search = '';
	h.goto.mockClear();
});

describe('metrics layout', () => {
	it('renders the window picker and both sub-tab links', async () => {
		mount();
		await expect.element(browser.getByRole('button', { name: '24h' })).toBeVisible();
		const nav = browser.getByRole('navigation', { name: 'Metrics sections' });
		await expect
			.element(nav.getByRole('link', { name: 'Usage' }))
			.toHaveAttribute('href', '/projects/alpha/metrics');
		await expect
			.element(nav.getByRole('link', { name: 'Step timing' }))
			.toHaveAttribute('href', '/projects/alpha/metrics/timing');
	});

	it('carries the window into the sub-tab links', async () => {
		h.search = '?window=30d';
		mount();
		await expect
			.element(browser.getByRole('link', { name: 'Step timing' }))
			.toHaveAttribute('href', '/projects/alpha/metrics/timing?window=30d');
	});

	it('marks the active sub-tab', async () => {
		mount();
		await expect
			.element(browser.getByRole('link', { name: 'Usage' }))
			.toHaveAttribute('aria-current', 'page');
		await expect
			.element(browser.getByRole('link', { name: 'Step timing' }))
			.not.toHaveAttribute('aria-current');
	});

	it('marks Step timing on the timing route', async () => {
		h.path = '/projects/alpha/metrics/timing';
		mount();
		await expect
			.element(browser.getByRole('link', { name: 'Step timing' }))
			.toHaveAttribute('aria-current', 'page');
		await expect
			.element(browser.getByRole('link', { name: 'Usage' }))
			.not.toHaveAttribute('aria-current');
	});

	it('warns about an invalid window', async () => {
		h.search = '?window=5y';
		mount();
		await expect.element(browser.getByText('Showing the last 7 days')).toBeVisible();
	});

	it('puts the chosen window in the URL', async () => {
		mount();
		await browser.getByRole('button', { name: '24h' }).click();
		expect(h.goto).toHaveBeenCalledWith(
			'?window=24h',
			expect.objectContaining({ replace: true, reset: false })
		);
	});
});
