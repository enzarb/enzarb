import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import NeedsYouRow from './NeedsYouRow.svelte';
import type { NeedsYou } from '#lib/work-items.ts';

const row = (needs: NeedsYou, state = 'AwaitingApproval') => ({
	id: 7,
	key: 'github:acme/web#7',
	project: 'web',
	workflow: 'issue',
	state,
	title: 'Add search',
	url: 'https://github.com/acme/web/issues/7',
	updatedAt: new Date(Date.now() - 3 * 3_600_000).toISOString(),
	needs
});

describe('NeedsYouRow', () => {
	it('shows project, key, age, GitHub link and the approve action', async () => {
		const onaction = vi.fn();
		render(NeedsYouRow, {
			item: row({ category: 'approval', reason: null }),
			onaction,
			loadPlan: async () => null
		});
		await expect.element(page.getByText('Plan awaiting approval')).toBeVisible();
		await expect.element(page.getByText('github:acme/web#7')).toBeVisible();
		await expect.element(page.getByText('waiting 3h')).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'View on GitHub' }))
			.toHaveAttribute('href', 'https://github.com/acme/web/issues/7');
		await page.getByRole('button', { name: 'Approve plan' }).click();
		expect(onaction).toHaveBeenCalledWith('approve');
	});

	it('offers retry with the failure reason for failed items', async () => {
		const onaction = vi.fn();
		render(NeedsYouRow, {
			item: row({ category: 'failed', reason: 'job timed out' }, 'Failed'),
			onaction,
			loadPlan: async () => null
		});
		await expect.element(page.getByText('job timed out')).toBeVisible();
		await page.getByRole('button', { name: 'Retry' }).click();
		expect(onaction).toHaveBeenCalledWith('retry');
	});

	it('asks for confirmation before merging', async () => {
		const onaction = vi.fn();
		render(NeedsYouRow, {
			item: row({ category: 'merge', reason: null }, 'ReadyToMerge'),
			onaction,
			loadPlan: async () => null
		});
		await page.getByRole('button', { name: 'Merge (squash)' }).click();
		expect(onaction).not.toHaveBeenCalled();
		await page.getByRole('button', { name: 'Merge (squash)' }).last().click();
		expect(onaction).toHaveBeenCalledWith('merge');
	});

	it('shows the blocker and no action for blocked items', async () => {
		render(NeedsYouRow, {
			item: row({ category: 'blocked', reason: 'CI: tests red' }, 'AwaitingChanges'),
			onaction: () => {},
			loadPlan: async () => null
		});
		await expect.element(page.getByText('CI: tests red')).toBeVisible();
		await expect.element(page.getByRole('button')).not.toBeInTheDocument();
	});
});
