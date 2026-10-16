import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import InstallBanner from './InstallBanner.svelte';

describe('InstallBanner', () => {
	it('links to the install page, explains the list and refreshes in place', async () => {
		const refresh = vi.fn(async () => {});
		render(InstallBanner, {
			installUrl: 'https://github.com/apps/my-app/installations/new',
			refresh
		});
		await expect
			.element(page.getByRole('link', { name: 'Install the GitHub App' }))
			.toHaveAttribute('href', 'https://github.com/apps/my-app/installations/new');
		await expect
			.element(
				page.getByText(/repositories the enzarb GitHub App can access where you have write access/)
			)
			.toBeVisible();
		await page.getByRole('button', { name: 'Refresh repositories' }).click();
		expect(refresh).toHaveBeenCalledOnce();
	});
});
