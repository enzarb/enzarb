import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import ConditionChecklist from './ConditionChecklist.svelte';

const cond = (type: string, status: string, reason = 'X', message = '') => ({
	type,
	status,
	reason,
	message
});

describe('ConditionChecklist', () => {
	it('renders each step with its state and message, in order', async () => {
		render(ConditionChecklist, {
			conditions: [
				cond('TenantReady', 'True'),
				cond('NamespaceReady', 'False', 'Creating', 'creating namespace')
			]
		});
		const items = page.getByRole('listitem');
		await expect.element(items.nth(0)).toHaveAttribute('data-state', 'done');
		await expect.element(items.nth(1)).toHaveAttribute('data-state', 'in-progress');
		await expect.element(items.nth(1)).toHaveTextContent('creating namespace');
		await expect.element(items.nth(2)).toHaveAttribute('data-state', 'pending');
		await expect.element(items.nth(3)).toHaveTextContent('Project ready');
	});

	it('shows failures with the condition message', async () => {
		render(ConditionChecklist, {
			conditions: [
				cond('TenantReady', 'True'),
				cond('NamespaceReady', 'False', 'NamespaceFailed', 'quota exceeded')
			]
		});
		const failed = page.getByRole('listitem').nth(1);
		await expect.element(failed).toHaveAttribute('data-state', 'failed');
		await expect.element(failed).toHaveTextContent('quota exceeded');
	});
});
