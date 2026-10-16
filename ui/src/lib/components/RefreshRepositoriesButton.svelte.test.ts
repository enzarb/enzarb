import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import RefreshRepositoriesButton from './RefreshRepositoriesButton.svelte';

describe('RefreshRepositoriesButton', () => {
	it('is busy while refreshing and calls refresh once per click', async () => {
		let release!: () => void;
		const refresh = vi.fn(() => new Promise<void>((r) => (release = r)));
		render(RefreshRepositoriesButton, { refresh });
		const button = page.getByRole('button', { name: 'Refresh repositories' });
		await button.click();
		expect(refresh).toHaveBeenCalledOnce();
		await expect.element(button).toBeDisabled();
		release();
		await expect.element(button).toBeEnabled();
	});

	it('says so when refreshing fails', async () => {
		render(RefreshRepositoriesButton, { refresh: async () => Promise.reject(new Error('x')) });
		await page.getByRole('button', { name: 'Refresh repositories' }).click();
		await expect.element(page.getByText('Could not refresh. Try again.')).toBeVisible();
	});
});
