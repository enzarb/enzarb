import { page, userEvent } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import TabsHarness from './__fixtures__/TabsHarness.svelte';

describe('Tabs', () => {
	it('selects the first tab and shows its panel', async () => {
		render(TabsHarness);
		await expect
			.element(page.getByRole('tab', { name: 'Alpha' }))
			.toHaveAttribute('aria-selected', 'true');
		await expect.element(page.getByRole('tabpanel')).toHaveTextContent('Panel a');
	});

	it('moves with arrow keys, Home and End', async () => {
		render(TabsHarness);
		page.getByRole('tab', { name: 'Alpha' }).element().focus();
		await userEvent.keyboard('{ArrowRight}');
		await expect.element(page.getByRole('tab', { name: 'Beta' })).toHaveFocus();
		await expect.element(page.getByRole('tabpanel')).toHaveTextContent('Panel b');
		await userEvent.keyboard('{End}');
		await expect.element(page.getByRole('tab', { name: 'Gamma' })).toHaveFocus();
		await userEvent.keyboard('{ArrowRight}');
		await expect.element(page.getByRole('tab', { name: 'Alpha' })).toHaveFocus();
		await userEvent.keyboard('{ArrowLeft}');
		await expect.element(page.getByRole('tab', { name: 'Gamma' })).toHaveFocus();
		await userEvent.keyboard('{Home}');
		await expect.element(page.getByRole('tab', { name: 'Alpha' })).toHaveFocus();
	});

	it('names the tab list', async () => {
		render(TabsHarness);
		await expect.element(page.getByRole('tablist', { name: 'Sections' })).toBeVisible();
	});
});
