import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import RulesProposal from './RulesProposal.svelte';
import type { RulesDiff } from '#lib/rules-diff.ts';

const diff: RulesDiff = {
	addedRules: ['review'],
	removedRules: [],
	changedRules: [],
	addedModes: [],
	removedModes: [],
	changedModes: [],
	summary: ['Added the rule "review": when code is pushed, run review']
};

describe('RulesProposal', () => {
	it('lists the changes and accepts or discards', async () => {
		const accept = vi.fn();
		const discard = vi.fn();
		render(RulesProposal, { diff, steps: [], busy: false, accept, discard });
		await expect.element(page.getByText(/Added the rule "review"/)).toBeVisible();
		await page.getByRole('button', { name: 'Accept and commit' }).click();
		await page.getByRole('button', { name: 'Discard' }).click();
		expect(accept).toHaveBeenCalledOnce();
		expect(discard).toHaveBeenCalledOnce();
	});

	it('disables both buttons while saving and shows the step checklist', async () => {
		render(RulesProposal, {
			diff,
			steps: [
				{ id: 'check', status: 'done', label: 'Check the changes' },
				{ id: 'rules', status: 'active', label: 'Commit .ez/workflows.toml to main' }
			],
			busy: true,
			accept: vi.fn(),
			discard: vi.fn()
		});
		await expect.element(page.getByRole('button', { name: 'Accept and commit' })).toBeDisabled();
		await expect.element(page.getByRole('button', { name: 'Discard' })).toBeDisabled();
		await expect.element(page.getByText('Commit .ez/workflows.toml to main')).toBeVisible();
	});
});
