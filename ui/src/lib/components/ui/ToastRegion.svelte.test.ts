import { page } from 'vitest/browser';
import { afterEach, describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { toasts } from '#lib/toast.svelte.ts';
import ToastRegion from './ToastRegion.svelte';

afterEach(() => toasts.clear());

describe('ToastRegion', () => {
	it('has its live regions in the page before anything is announced', async () => {
		render(ToastRegion);
		const status = page.getByRole('status');
		await expect.element(status).toHaveAttribute('aria-live', 'polite');
		// the alert block only exists while an error is showing
		await expect.element(page.getByRole('alert')).not.toBeInTheDocument();
		await expect.element(status).not.toHaveTextContent(/\S/);
	});

	it('announces info and success politely, errors assertively', async () => {
		render(ToastRegion);
		toasts.push('Saved', { tone: 'success', description: 'All good' });
		toasts.push('Could not save', { tone: 'error' });
		await expect.element(page.getByRole('status')).toHaveTextContent('Saved');
		await expect.element(page.getByRole('status')).toHaveTextContent('All good');
		await expect.element(page.getByRole('status')).not.toHaveTextContent('Could not save');
		await expect.element(page.getByRole('alert')).toHaveTextContent('Could not save');
	});

	it('dismisses by button and by timeout', async () => {
		render(ToastRegion);
		toasts.push('One', { timeout: 0 });
		await page.getByRole('button', { name: 'Dismiss notification' }).click();
		await expect.element(page.getByRole('status')).not.toHaveTextContent('One');
		toasts.push('Two', { timeout: 30 });
		await expect.element(page.getByRole('status')).toHaveTextContent('Two');
		await expect.poll(() => toasts.items.length).toBe(0);
	});
});
