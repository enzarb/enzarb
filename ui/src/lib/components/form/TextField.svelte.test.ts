import { page, userEvent } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import TextField from './TextField.svelte';
import TextArea from './TextArea.svelte';

describe('TextField', () => {
	it('labels the input and links help and errors', async () => {
		render(TextField, { label: 'Name', help: 'Lowercase', issues: [{ message: 'too short' }] });
		const input = page.getByRole('textbox', { name: 'Name' });
		await expect.element(input).toHaveAttribute('aria-invalid', 'true');
		const described = (await input.element().getAttribute('aria-describedby'))!.split(' ');
		expect(described.map((id) => document.getElementById(id)?.textContent?.trim())).toEqual([
			'Lowercase',
			'too short'
		]);
	});

	it('is not invalid without issues and accepts typing', async () => {
		render(TextField, { label: 'Name', name: 'n' });
		const input = page.getByRole('textbox', { name: 'Name' });
		await expect.element(input).not.toHaveAttribute('aria-invalid');
		await input.fill('hello');
		await expect.element(input).toHaveValue('hello');
		await expect.element(input).toHaveAttribute('name', 'n');
	});

	it('TextArea is a labelled multiline textbox', async () => {
		render(TextArea, { label: 'Notes', issues: ['bad'] });
		const area = page.getByRole('textbox', { name: 'Notes' });
		await userEvent.type(area, 'a');
		await expect.element(area).toHaveValue('a');
		await expect.element(area).toHaveAttribute('aria-invalid', 'true');
		await expect.element(page.getByText('bad')).toBeInTheDocument();
	});
});
