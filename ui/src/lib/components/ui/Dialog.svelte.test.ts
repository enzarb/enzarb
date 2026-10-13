import { page, userEvent } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import DialogHarness from './__fixtures__/DialogHarness.svelte';

describe('Dialog', () => {
	it('opens as a named dialog and moves focus inside', async () => {
		render(DialogHarness);
		await page.getByRole('button', { name: 'Open' }).click();
		const dialog = page.getByRole('dialog', { name: 'Edit thing' });
		await expect.element(dialog).toBeVisible();
		await expect.element(dialog).toHaveAccessibleDescription('Make a change.');
		expect(dialog.element().contains(document.activeElement)).toBe(true);
	});

	it('keeps Tab focus inside the dialog', async () => {
		render(DialogHarness);
		await page.getByRole('button', { name: 'Open' }).click();
		const dialog = page.getByRole('dialog', { name: 'Edit thing' });
		await expect.element(dialog).toBeVisible();
		for (let i = 0; i < 8; i++) {
			await userEvent.tab();
			expect(dialog.element().contains(document.activeElement)).toBe(true);
		}
		for (let i = 0; i < 4; i++) {
			await userEvent.tab({ shift: true });
			expect(dialog.element().contains(document.activeElement)).toBe(true);
		}
	});

	it('closes on Escape and returns focus to the opener', async () => {
		render(DialogHarness);
		const opener = page.getByRole('button', { name: 'Open' });
		await opener.click();
		await expect.element(page.getByRole('dialog')).toBeVisible();
		await userEvent.keyboard('{Escape}');
		await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
		await expect.element(opener).toHaveFocus();
	});
});
