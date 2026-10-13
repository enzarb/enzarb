import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import ConfirmDialog from './ConfirmDialog.svelte';

describe('ConfirmDialog', () => {
	it('calls onconfirm and closes', async () => {
		const onconfirm = vi.fn();
		render(ConfirmDialog, { open: true, title: 'Delete it?', confirmLabel: 'Delete', onconfirm });
		await expect.element(page.getByRole('dialog', { name: 'Delete it?' })).toBeVisible();
		await page.getByRole('button', { name: 'Delete' }).click();
		expect(onconfirm).toHaveBeenCalledOnce();
		await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
	});

	it('cancel closes without confirming', async () => {
		const onconfirm = vi.fn();
		render(ConfirmDialog, { open: true, title: 'Delete it?', onconfirm });
		await page.getByRole('button', { name: 'Cancel' }).click();
		await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
		expect(onconfirm).not.toHaveBeenCalled();
	});
});
