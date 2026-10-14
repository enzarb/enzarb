import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import UserMenu from './UserMenu.svelte';

describe('UserMenu', () => {
	it('offers sign-out and the theme group in the menu', async () => {
		render(UserMenu, { login: 'ruckc', name: 'Curtis', avatarUrl: null });
		await page.getByRole('button', { name: /Account menu/ }).click();
		await expect.element(page.getByRole('menuitem', { name: 'Sign out' })).toBeVisible();
		await expect.element(page.getByRole('group', { name: 'Theme' })).toBeVisible();
	});

	it('links to Settings before Sign out', async () => {
		render(UserMenu, { login: 'ruckc', name: 'Curtis', avatarUrl: null });
		await page.getByRole('button', { name: /Account menu/ }).click();
		const settings = page.getByRole('menuitem', { name: 'Settings' });
		await expect.element(settings).toHaveAttribute('href', '/settings');
		const labels = Array.from(document.querySelectorAll('[role="menuitem"]')).map((el) =>
			el.textContent?.trim()
		);
		expect(labels.indexOf('Settings')).toBeGreaterThanOrEqual(0);
		expect(labels.indexOf('Settings')).toBeLessThan(labels.indexOf('Sign out'));
	});
});
