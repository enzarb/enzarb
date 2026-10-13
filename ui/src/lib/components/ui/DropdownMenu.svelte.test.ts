import { page, userEvent } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import MenuHarness from './__fixtures__/MenuHarness.svelte';

describe('DropdownMenu', () => {
	it('opens with the keyboard and moves with arrow keys', async () => {
		render(MenuHarness);
		const trigger = page.getByRole('button', { name: 'Open menu' });
		trigger.element().focus();
		await userEvent.keyboard('{Enter}');
		await expect.element(page.getByRole('menu')).toBeVisible();
		// opening with the keyboard focuses the first item
		await expect.element(page.getByRole('menuitem', { name: 'First' })).toHaveFocus();
		await userEvent.keyboard('{ArrowDown}');
		await expect.element(page.getByRole('menuitem', { name: 'Second' })).toHaveFocus();
		await userEvent.keyboard('{ArrowUp}');
		await expect.element(page.getByRole('menuitem', { name: 'First' })).toHaveFocus();
	});

	it('selects with Enter, closes and restores focus', async () => {
		render(MenuHarness);
		const trigger = page.getByRole('button', { name: 'Open menu' });
		trigger.element().focus();
		await userEvent.keyboard('{Enter}');
		await userEvent.keyboard('{ArrowDown}{Enter}');
		await expect.element(page.getByText('picked: second')).toBeVisible();
		await expect.element(page.getByRole('menu')).not.toBeInTheDocument();
		await expect.element(trigger).toHaveFocus();
	});

	it('closes on Escape', async () => {
		render(MenuHarness);
		await page.getByRole('button', { name: 'Open menu' }).click();
		await expect.element(page.getByRole('menu')).toBeVisible();
		await userEvent.keyboard('{Escape}');
		await expect.element(page.getByRole('menu')).not.toBeInTheDocument();
	});

	it('marks the chosen radio item', async () => {
		render(MenuHarness);
		await page.getByRole('button', { name: 'Open menu' }).click();
		await expect.element(page.getByRole('menuitemradio', { name: 'System' })).toBeChecked();
		await expect.element(page.getByRole('group', { name: 'Appearance' })).toBeVisible();
		await expect.element(page.getByRole('menuitemradio', { name: 'Dark' })).not.toBeChecked();
		await page.getByRole('menuitemradio', { name: 'Dark' }).click();
		await expect.element(page.getByRole('menuitemradio', { name: 'Dark' })).toBeChecked();
	});
});
