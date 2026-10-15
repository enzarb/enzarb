import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { defaultWindow } from '#lib/metrics-window.ts';
import WindowPicker from './WindowPicker.svelte';

const value = defaultWindow(new Date());

describe('WindowPicker', () => {
	it('marks the current preset and reports a click as URL params', async () => {
		const onchange = vi.fn();
		render(WindowPicker, { value, onchange });
		await expect
			.element(page.getByRole('button', { name: '7d' }))
			.toHaveAttribute('aria-pressed', 'true');
		await page.getByRole('button', { name: '30d' }).click();
		expect(onchange).toHaveBeenCalledOnce();
		expect(onchange.mock.calls[0][0].toString()).toBe('window=30d');
	});

	it('applies a valid custom range', async () => {
		const onchange = vi.fn();
		render(WindowPicker, { value, onchange });
		await page.getByLabelText('From').fill('2026-01-01');
		await page.getByLabelText('To').fill('2026-01-31');
		await page.getByRole('button', { name: 'Apply' }).click();
		expect(onchange.mock.calls[0][0].toString()).toBe('from=2026-01-01&to=2026-01-31');
	});

	it('shows an error and does not navigate on an invalid range', async () => {
		const onchange = vi.fn();
		render(WindowPicker, { value, onchange });
		await page.getByLabelText('From').fill('2026-02-01');
		await page.getByLabelText('To').fill('2026-01-01');
		await page.getByRole('button', { name: 'Apply' }).click();
		await expect
			.element(page.getByRole('alert'))
			.toHaveTextContent('The start must be before the end.');
		expect(onchange).not.toHaveBeenCalled();
	});

	it('asks for both dates', async () => {
		const onchange = vi.fn();
		render(WindowPicker, { value, onchange });
		await page.getByRole('button', { name: 'Apply' }).click();
		await expect.element(page.getByRole('alert')).toBeVisible();
		expect(onchange).not.toHaveBeenCalled();
	});
});
