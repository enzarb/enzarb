import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import MetricsNav from './MetricsNav.svelte';

describe('MetricsNav', () => {
	it('keeps the query string on every link', async () => {
		render(MetricsNav, {
			name: 'alpha',
			pathname: '/projects/alpha/metrics',
			search: '?window=24h'
		});
		await expect
			.element(page.getByRole('link', { name: 'Usage' }))
			.toHaveAttribute('href', '/projects/alpha/metrics?window=24h');
		await expect
			.element(page.getByRole('link', { name: 'Step timing' }))
			.toHaveAttribute('href', '/projects/alpha/metrics/timing?window=24h');
		await expect
			.element(page.getByRole('link', { name: 'Sandbox' }))
			.toHaveAttribute('href', '/projects/alpha/metrics/sandbox?window=24h');
	});

	it('marks Usage current on the index only', async () => {
		render(MetricsNav, { name: 'alpha', pathname: '/projects/alpha/metrics', search: '' });
		await expect
			.element(page.getByRole('link', { name: 'Usage' }))
			.toHaveAttribute('aria-current', 'page');
		await expect
			.element(page.getByRole('link', { name: 'Step timing' }))
			.not.toHaveAttribute('aria-current');
	});

	it('marks Step timing current on its route', async () => {
		render(MetricsNav, { name: 'alpha', pathname: '/projects/alpha/metrics/timing', search: '' });
		await expect
			.element(page.getByRole('link', { name: 'Step timing' }))
			.toHaveAttribute('aria-current', 'page');
		await expect
			.element(page.getByRole('link', { name: 'Usage' }))
			.not.toHaveAttribute('aria-current');
	});
});
