import { page, userEvent } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import Select from './Select.svelte';

const options = [
	{ value: 'a', label: 'Alpha' },
	{ value: 'b', label: 'Beta' },
	{ value: 'c', label: 'Gamma', disabled: true, description: 'in use' }
];

describe('Select', () => {
	it('exposes a labelled listbox trigger and selects with the keyboard', async () => {
		const seen: string[] = [];
		render(Select, { label: 'Letter', options, value: 'a', onchange: (v: string) => seen.push(v) });
		const trigger = page.getByRole('button', { name: /Letter/ });
		await expect.element(trigger).toHaveAttribute('aria-haspopup', 'listbox');
		await expect.element(trigger).toHaveAttribute('aria-expanded', 'false');
		await expect.element(trigger).toHaveTextContent('Alpha');

		await trigger.click();
		await expect.element(trigger).toHaveAttribute('aria-expanded', 'true');
		await expect.element(page.getByRole('listbox')).toBeInTheDocument();
		await expect
			.element(page.getByRole('option', { name: 'Alpha' }))
			.toHaveAttribute('aria-selected', 'true');
		await expect
			.element(page.getByRole('option', { name: /Gamma/ }))
			.toHaveAttribute('aria-disabled', 'true');

		await userEvent.keyboard('{ArrowDown}');
		await expect
			.element(page.getByRole('option', { name: 'Beta' }))
			.toHaveAttribute('data-highlighted');
		await userEvent.keyboard('{Enter}');
		await expect.element(trigger).toHaveTextContent('Beta');
		expect(seen).toEqual(['b']);
		await expect.element(page.getByRole('listbox')).not.toBeInTheDocument();
	});

	it('closes on Escape, jumps with Home/End and typeahead', async () => {
		render(Select, { label: 'Letter', options, value: 'a' });
		const trigger = page.getByRole('button', { name: /Letter/ });
		await trigger.click();
		await userEvent.keyboard('{Escape}');
		await expect.element(page.getByRole('listbox')).not.toBeInTheDocument();

		await userEvent.keyboard('{Enter}');
		await userEvent.keyboard('{End}');
		await expect.element(page.getByRole('option', { name: 'Beta' })).toBeVisible();
		await userEvent.keyboard('{Home}{Enter}');
		await expect.element(trigger).toHaveTextContent('Alpha');

		await userEvent.keyboard('{Enter}b{Enter}');
		await expect.element(trigger).toHaveTextContent('Beta');
	});

	it('selecting an option with an empty value works', async () => {
		render(Select, {
			label: 'Project',
			value: 'x',
			options: [
				{ value: '', label: 'All' },
				{ value: 'x', label: 'X' }
			]
		});
		const trigger = page.getByRole('button', { name: 'Project' });
		await trigger.click();
		await page.getByRole('option', { name: 'All' }).click();
		await expect.element(trigger).toHaveTextContent('All');
	});
});
