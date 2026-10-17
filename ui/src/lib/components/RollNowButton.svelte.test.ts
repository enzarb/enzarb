import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import RollNowButton from './RollNowButton.svelte';

describe('RollNowButton', () => {
	it('warns about interruption and rolls for the displayed generation on confirm', async () => {
		const roll = vi.fn().mockResolvedValue(undefined);
		render(RollNowButton, { generation: 3, roll });
		await page.getByRole('button', { name: 'Roll now' }).click();
		await expect.element(page.getByText(/interrupted/)).toBeVisible();
		expect(roll).not.toHaveBeenCalled();
		await page.getByRole('button', { name: 'Roll now' }).nth(1).click();
		expect(roll).toHaveBeenCalledExactlyOnceWith(3);
		await expect.element(page.getByText('Restart requested')).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Restart requested' })).toBeDisabled();
	});

	it('does nothing on cancel', async () => {
		const roll = vi.fn();
		render(RollNowButton, { generation: 1, roll });
		await page.getByRole('button', { name: 'Roll now' }).click();
		await page.getByRole('button', { name: 'Cancel' }).click();
		expect(roll).not.toHaveBeenCalled();
	});

	it('offers a retry when the server refuses', async () => {
		const roll = vi.fn().mockRejectedValue(new Error('409'));
		render(RollNowButton, { generation: 1, roll });
		await page.getByRole('button', { name: 'Roll now' }).click();
		await page.getByRole('button', { name: 'Roll now' }).nth(1).click();
		await expect.element(page.getByRole('status')).toHaveTextContent(/moved on/);
		await expect.element(page.getByRole('button', { name: 'Roll now' }).first()).toBeEnabled();
	});
});
