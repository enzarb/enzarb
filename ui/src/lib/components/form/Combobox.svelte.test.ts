import { page, userEvent } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import Combobox from './Combobox.svelte';

const options = [
	{ value: 'o/one', label: 'one', group: 'octo' },
	{ value: 'o/two', label: 'two', group: 'octo', disabled: true, description: 'in project X' },
	{ value: 'm/three', label: 'three', group: 'mona' }
];

describe('Combobox', () => {
	it('has the combobox/listbox ARIA wiring and groups', async () => {
		render(Combobox, { label: 'Repositories', options });
		const input = page.getByRole('combobox', { name: 'Repositories' });
		await expect.element(input).toHaveAttribute('aria-expanded', 'false');
		await input.click();
		await userEvent.keyboard('{ArrowDown}');
		await expect.element(input).toHaveAttribute('aria-expanded', 'true');
		await expect.element(page.getByRole('listbox')).toHaveAttribute('aria-multiselectable', 'true');
		await expect.element(page.getByRole('group', { name: 'octo' })).toBeInTheDocument();
		await expect.element(page.getByRole('group', { name: 'mona' })).toBeInTheDocument();
		const two = page.getByRole('option', { name: /two/ });
		await expect.element(two).toHaveAttribute('aria-disabled', 'true');
		await expect.element(two).toHaveTextContent('in project X');
	});

	it('filters as you type and selects several with the keyboard, showing chips', async () => {
		render(Combobox, { label: 'Repositories', options });
		const input = page.getByRole('combobox', { name: 'Repositories' });
		await input.click();
		await userEvent.keyboard('thr');
		await expect.element(page.getByRole('option', { name: /one/ })).not.toBeInTheDocument();
		await userEvent.keyboard('{ArrowDown}');
		await expect
			.element(page.getByRole('option', { name: 'three' }))
			.toHaveAttribute('data-highlighted');
		await userEvent.keyboard('{Enter}');
		await expect.element(page.getByRole('button', { name: 'Remove three' })).toBeInTheDocument();
		await expect
			.element(page.getByRole('option', { name: 'three' }))
			.toHaveAttribute('aria-selected', 'true');
	});

	it('selects several options by click, skips disabled ones, removes via chip and Backspace', async () => {
		render(Combobox, { label: 'Repositories', options });
		const input = page.getByRole('combobox', { name: 'Repositories' });
		await input.click();
		await userEvent.keyboard('{ArrowDown}');
		await page.getByRole('option', { name: 'one' }).click();
		await page.getByRole('option', { name: 'three' }).click();
		await expect.element(page.getByRole('button', { name: 'Remove one' })).toBeInTheDocument();
		await expect.element(page.getByRole('button', { name: 'Remove three' })).toBeInTheDocument();

		await page.getByRole('option', { name: /two/ }).click({ force: true });
		await expect.element(page.getByRole('button', { name: 'Remove two' })).not.toBeInTheDocument();

		await userEvent.keyboard('{Escape}');
		await expect.element(page.getByRole('listbox')).not.toBeInTheDocument();
		await input.click();
		await userEvent.keyboard('{Backspace}');
		await expect
			.element(page.getByRole('button', { name: 'Remove three' }))
			.not.toBeInTheDocument();
		await page.getByRole('button', { name: 'Remove one' }).click();
		await expect.element(page.getByRole('button', { name: 'Remove one' })).not.toBeInTheDocument();
	});

	it('after selecting, Backspace removes only the chip and the input stays empty', async () => {
		render(Combobox, { label: 'Repositories', options });
		const input = page.getByRole('combobox', { name: 'Repositories' });
		await input.click();
		await userEvent.keyboard('thr');
		await expect.element(page.getByRole('option', { name: 'three' })).toBeVisible();
		await userEvent.keyboard('{ArrowDown}');
		await expect
			.element(page.getByRole('option', { name: 'three' }))
			.toHaveAttribute('data-highlighted');
		await userEvent.keyboard('{Enter}');
		await expect.element(page.getByRole('button', { name: 'Remove three' })).toBeInTheDocument();
		await expect.element(input).toHaveValue('');
		await userEvent.keyboard('{Backspace}');
		await expect
			.element(page.getByRole('button', { name: 'Remove three' }))
			.not.toBeInTheDocument();
		await expect.element(input).toHaveValue('');
	});

	it('clears typed text on Escape', async () => {
		render(Combobox, { label: 'Repositories', options });
		const input = page.getByRole('combobox', { name: 'Repositories' });
		await input.click();
		await userEvent.keyboard('thr{Escape}');
		await expect.element(input).toHaveValue('');
	});

	it('keeps focus and announces when a chip is removed from the keyboard', async () => {
		render(Combobox, { label: 'Repositories', options, value: ['o/one', 'm/three'] });
		const first = page.getByRole('button', { name: 'Remove one' });
		(first.element() as HTMLElement).focus();
		await userEvent.keyboard('{Enter}');
		await expect.element(page.getByText('one removed')).toBeInTheDocument();
		const next = page.getByRole('button', { name: 'Remove three' });
		await expect.element(next).toHaveFocus();
		await userEvent.keyboard('{Enter}');
		await expect.element(page.getByRole('combobox', { name: 'Repositories' })).toHaveFocus();
	});

	it('shows an empty state', async () => {
		render(Combobox, { label: 'Repositories', options, emptyText: 'nothing' });
		await page.getByRole('combobox', { name: 'Repositories' }).click();
		await userEvent.keyboard('zzz');
		await expect.element(page.getByText('nothing')).toBeInTheDocument();
	});
});
