import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import WorkActions from './WorkActions.svelte';

describe('WorkActions', () => {
	it('renders only the given actions and reports clicks', async () => {
		const onaction = vi.fn();
		render(WorkActions, { actions: ['approve', 'cancel'], onaction });
		await expect.element(page.getByRole('button', { name: 'Retry' })).not.toBeInTheDocument();
		await page.getByRole('button', { name: 'Approve plan' }).click();
		expect(onaction).toHaveBeenCalledWith('approve');
	});
	it('renders Merge (squash) as a primary button and reports the click', async () => {
		const onaction = vi.fn();
		render(WorkActions, { actions: ['merge', 'cancel'], onaction });
		const button = page.getByRole('button', { name: 'Merge (squash)' });
		await expect.element(button).toHaveClass(/primary/);
		await button.click();
		expect(onaction).toHaveBeenCalledWith('merge');
	});
	it('renders nothing for terminal states and disables buttons while pending', async () => {
		const r = render(WorkActions, { actions: [], onaction: () => {} });
		await expect.element(page.getByRole('button')).not.toBeInTheDocument();
		r.unmount();
		render(WorkActions, { actions: ['cancel'], pending: 'cancel', onaction: () => {} });
		await expect.element(page.getByRole('button', { name: 'Cancel' })).toBeDisabled();
	});
});
