import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import WaitButtons from './WaitButtons.svelte';

describe('WaitButtons', () => {
	it('renders the declared buttons and reports the command of a click', async () => {
		const oncommand = vi.fn();
		render(WaitButtons, {
			buttons: [
				{ label: 'Start planning', command: 'plan' },
				{ label: 'Skip', command: 'skip' }
			],
			oncommand
		});
		const start = page.getByRole('button', { name: 'Start planning' });
		await expect.element(start).toHaveClass(/primary/);
		await expect.element(start).toHaveAttribute('title', 'Posts /plan as you');
		await page.getByRole('button', { name: 'Skip' }).click();
		expect(oncommand).toHaveBeenCalledWith('skip');
	});
	it('renders nothing without buttons and disables them while pending', async () => {
		const r = render(WaitButtons, { buttons: [], oncommand: () => {} });
		await expect.element(page.getByRole('button')).not.toBeInTheDocument();
		r.unmount();
		render(WaitButtons, {
			buttons: [{ label: 'Start planning', command: 'plan' }],
			disabled: true,
			oncommand: () => {}
		});
		await expect.element(page.getByRole('button', { name: 'Start planning' })).toBeDisabled();
	});
});
