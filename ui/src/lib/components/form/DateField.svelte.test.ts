import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import DateField from './DateField.svelte';

describe('DateField', () => {
	it('renders a labelled date input styled as a control', async () => {
		render(DateField, { label: 'From' });
		const input = page.getByLabelText('From');
		await expect.element(input).toHaveAttribute('type', 'date');
		await expect.element(input).toHaveClass('ez-control');
		await expect.element(input).not.toHaveAttribute('aria-invalid');
	});

	it('takes and reflects YYYY-MM-DD values and passes min/max through', async () => {
		render(DateField, { label: 'To', value: '2026-01-02', min: '2026-01-01', max: '2026-12-31' });
		const input = page.getByLabelText('To');
		await expect.element(input).toHaveValue('2026-01-02');
		await expect.element(input).toHaveAttribute('min', '2026-01-01');
		await expect.element(input).toHaveAttribute('max', '2026-12-31');
		await input.fill('2026-03-04');
		await expect.element(input).toHaveValue('2026-03-04');
	});

	it('links help and issues and marks the input invalid', async () => {
		render(DateField, { label: 'From', help: 'Start day', issues: [{ message: 'too early' }] });
		const input = page.getByLabelText('From');
		await expect.element(input).toHaveAttribute('aria-invalid', 'true');
		const described = (await input.element().getAttribute('aria-describedby'))!.split(' ');
		expect(described.map((id) => document.getElementById(id)?.textContent?.trim())).toEqual([
			'Start day',
			'too early'
		]);
	});
});
