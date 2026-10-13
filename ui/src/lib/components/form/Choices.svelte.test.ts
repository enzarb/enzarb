import { page, userEvent } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import Checkbox from './Checkbox.svelte';
import RadioGroup from './RadioGroup.svelte';
import Switch from './Switch.svelte';

describe('Checkbox', () => {
	it('is a labelled checkbox toggled by Space and by clicking the label', async () => {
		render(Checkbox, { label: 'Issues', help: 'Open issues' });
		const box = page.getByRole('checkbox', { name: 'Issues' });
		await expect.element(box).toHaveAttribute('aria-checked', 'false');
		(box.element() as HTMLElement).focus();
		await userEvent.keyboard(' ');
		await expect.element(box).toHaveAttribute('aria-checked', 'true');
		await page.getByText('Issues', { exact: true }).click();
		await expect.element(box).toHaveAttribute('aria-checked', 'false');
		await expect.element(box).toHaveAccessibleDescription('Open issues');
	});

	it('supports disabled and error state', async () => {
		render(Checkbox, { label: 'Mentions', disabled: true, issues: ['required'] });
		const box = page.getByRole('checkbox', { name: 'Mentions' });
		await expect.element(box).toBeDisabled();
		await expect.element(box).toHaveAttribute('aria-invalid', 'true');
		await expect.element(page.getByText('required')).toBeInTheDocument();
	});
});

describe('Switch', () => {
	it('is a labelled switch toggled by Space and Enter', async () => {
		render(Switch, { label: 'Live' });
		const sw = page.getByRole('switch', { name: 'Live' });
		await expect.element(sw).toHaveAttribute('aria-checked', 'false');
		(sw.element() as HTMLElement).focus();
		await userEvent.keyboard(' ');
		await expect.element(sw).toHaveAttribute('aria-checked', 'true');
		await userEvent.keyboard('{Enter}');
		await expect.element(sw).toHaveAttribute('aria-checked', 'false');
	});
});

describe('RadioGroup', () => {
	const options = [
		{ value: 'a', label: 'Alpha' },
		{ value: 'b', label: 'Beta', disabled: true },
		{ value: 'c', label: 'Gamma' }
	];

	it('is a labelled radiogroup with arrow-key selection that skips disabled items', async () => {
		render(RadioGroup, { label: 'Letter', options, value: 'a' });
		await expect.element(page.getByRole('radiogroup', { name: 'Letter' })).toBeInTheDocument();
		const alpha = page.getByRole('radio', { name: 'Alpha' });
		const gamma = page.getByRole('radio', { name: 'Gamma' });
		await expect.element(alpha).toHaveAttribute('aria-checked', 'true');
		(alpha.element() as HTMLElement).focus();
		await userEvent.keyboard('{ArrowDown}');
		await expect.element(gamma).toHaveAttribute('aria-checked', 'true');
		await expect.element(alpha).toHaveAttribute('aria-checked', 'false');
		await userEvent.keyboard('{ArrowDown}');
		await expect.element(alpha).toHaveAttribute('aria-checked', 'true');
		await expect.element(page.getByRole('radio', { name: 'Beta' })).toBeDisabled();
	});
});
