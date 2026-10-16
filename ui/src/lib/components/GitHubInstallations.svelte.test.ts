import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import GitHubInstallations from './GitHubInstallations.svelte';

const base = {
	installUrl: 'https://github.com/apps/my-app/installations/new',
	refresh: async () => {}
};

describe('GitHubInstallations', () => {
	it('links to the install page in a new tab and refreshes in place', async () => {
		const refresh = vi.fn(async () => {});
		render(GitHubInstallations, { ...base, refresh, installations: [], fetchedAt: null });
		const install = page.getByRole('link', { name: /Install \/ configure the GitHub App/ });
		await expect.element(install).toHaveAttribute('href', base.installUrl);
		await expect.element(install).toHaveAttribute('target', '_blank');
		await page.getByRole('button', { name: 'Refresh repositories' }).click();
		expect(refresh).toHaveBeenCalledOnce();
	});

	it('lists accounts with repository counts', async () => {
		render(GitHubInstallations, {
			...base,
			installations: [
				{ accountLogin: 'acme', installationId: 7, repositoryCount: 3 },
				{ accountLogin: 'me', installationId: 9, repositoryCount: 1 }
			],
			fetchedAt: '2026-10-11T10:00:00.000Z'
		});
		await expect.element(page.getByText('acme')).toBeVisible();
		await expect.element(page.getByText(/3 repositories \(installation 7\)/)).toBeVisible();
		await expect.element(page.getByText(/1 repository \(installation 9\)/)).toBeVisible();
		await expect.element(page.getByText('2026-10-11T10:00:00.000Z')).toBeVisible();
	});

	it('renders the empty state and a missing fetch time', async () => {
		render(GitHubInstallations, { ...base, installations: [], fetchedAt: null });
		await expect.element(page.getByText(/No installations found/)).toBeVisible();
		await expect.element(page.getByText('Never')).toBeVisible();
	});
});
