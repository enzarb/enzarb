import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { completeStep, failCurrent, initialProgress } from '#lib/terminal-steps.ts';
import TerminalChecklist from './TerminalChecklist.svelte';

describe('TerminalChecklist', () => {
	it('shows done, in-progress and pending steps in order', async () => {
		render(TerminalChecklist, { progress: completeStep(initialProgress, 'token') });
		const items = page.getByRole('listitem');
		await expect.element(items.nth(0)).toHaveAttribute('data-state', 'done');
		await expect.element(items.nth(1)).toHaveAttribute('data-state', 'in-progress');
		await expect.element(items.nth(3)).toHaveAttribute('data-state', 'pending');
	});

	it('shows the failure message on the failed step', async () => {
		const progress = failCurrent(completeStep(initialProgress, 'token'), 'connection refused');
		render(TerminalChecklist, { progress });
		const failed = page.getByRole('listitem').nth(1);
		await expect.element(failed).toHaveAttribute('data-state', 'failed');
		await expect.element(failed).toHaveTextContent('connection refused');
	});
});
