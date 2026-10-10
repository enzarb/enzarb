import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import InstallBanner from './InstallBanner.svelte';

describe('InstallBanner', () => {
	it('links to the install page and to the refresh flow', async () => {
		render(InstallBanner, {
			installUrl: 'https://github.com/apps/my-app/installations/new',
			refreshHref: '/login/github?refresh=1&return=%2Fprojects'
		});
		await expect
			.element(page.getByRole('link', { name: 'Install the GitHub App' }))
			.toHaveAttribute('href', 'https://github.com/apps/my-app/installations/new');
		await expect
			.element(page.getByRole('link', { name: 'Refresh repositories' }))
			.toHaveAttribute('href', '/login/github?refresh=1&return=%2Fprojects');
		await expect.element(page.getByRole('status')).toBeVisible();
	});
});
