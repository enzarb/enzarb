import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import NeedsYouRow from './NeedsYouRow.svelte';
import type { NeedsYou, WorkItemSummary } from '#lib/work-items.ts';

const row = (
	needs: NeedsYou,
	state = 'AwaitingApproval',
	extra: { url?: string; singleRepo?: boolean; wait?: WorkItemSummary['wait'] } = {}
) => ({
	id: 7,
	key: 'github:acme/web#7',
	project: 'web',
	workflow: 'issue',
	state,
	title: 'Add search',
	url: 'https://github.com/acme/web/issues/7',
	updatedAt: new Date(Date.now() - 3 * 3_600_000).toISOString(),
	needs,
	...extra
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
		const ref = page.getByRole('link', { name: 'acme/web#7' });
		await expect.element(ref).toHaveAttribute('href', 'https://github.com/acme/web/issues/7');
		await expect.element(ref).toHaveAttribute('target', '_blank');
		await expect.element(ref).toHaveAttribute('rel', expect.stringContaining('noopener'));
		await expect
			.element(page.getByRole('link', { name: 'Add search' }))
			.not.toHaveAttribute('target');
		await page.getByRole('button', { name: 'Approve plan' }).click();
		expect(onaction).toHaveBeenCalledWith('approve');
	});

	it('shows just #n for single-repo projects', async () => {
		render(NeedsYouRow, {
			item: row({ category: 'approval', reason: null }, 'AwaitingApproval', { singleRepo: true }),
			onaction: vi.fn(),
			loadPlan: async () => null
		});
		await expect.element(page.getByRole('link', { name: '#7' })).toBeVisible();
	});

	it('falls back to "View on GitHub" for an unparseable URL', async () => {
		render(NeedsYouRow, {
			item: row({ category: 'approval', reason: null }, 'AwaitingApproval', {
				url: 'https://example.com/x'
			}),
			onaction: vi.fn(),
			loadPlan: async () => null
		});
		await expect.element(page.getByRole('link', { name: 'View on GitHub' })).toBeVisible();
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

	it('lists the unverified checks and links to the review for a change request', async () => {
		const text = 'enzarb could not verify: connectors tests (missing_tool)';
		render(NeedsYouRow, {
			item: {
				...row({ category: 'review', reason: null, text }, 'AwaitingChanges'),
				workflow: 'change_request',
				url: 'https://github.com/acme/web/pull/7',
				wait: { waitingOn: 'human', reason: 'checks_unverified', text, url: null }
			},
			onaction: vi.fn(),
			loadPlan: async () => null
		});
		await expect.element(page.getByText(text)).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'Review on GitHub' }))
			.toHaveAttribute('href', 'https://github.com/acme/web/pull/7/files');
	});

	it('shows an unknown wait reason as its text without an action', async () => {
		render(NeedsYouRow, {
			item: {
				...row({ category: 'blocked', reason: null, text: 'Something new' }, 'AwaitingChanges'),
				workflow: 'change_request',
				wait: { waitingOn: 'human', reason: 'from_the_future', text: 'Something new', url: null }
			},
			onaction: vi.fn(),
			loadPlan: async () => null
		});
		await expect.element(page.getByText('Something new')).toBeVisible();
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

	const PR = 'https://github.com/acme/web/pull/7';
	const crRow = (needs: NeedsYou, reason: string, extra = {}) => ({
		...row(needs, 'AwaitingChanges'),
		workflow: 'change_request',
		url: PR,
		wait: { waitingOn: 'human' as const, reason, text: needs.text ?? '', url: null, ...extra }
	});

	it('shows the reason text and a Review on GitHub button to the files tab', async () => {
		render(NeedsYouRow, {
			item: crRow(
				{ category: 'review', reason: null, text: 'Waiting for your review' },
				'awaiting_review'
			),
			onaction: () => {},
			loadPlan: async () => null
		});
		await expect.element(page.getByText('Waiting for your review')).toBeVisible();
		await expect.element(page.getByText('Blocked')).not.toBeInTheDocument();
		await expect
			.element(page.getByRole('link', { name: 'Review on GitHub' }))
			.toHaveAttribute('href', `${PR}/files`);
	});

	it('offers Open PR next to nothing else for a parked conflict', async () => {
		render(NeedsYouRow, {
			item: crRow(
				{ category: 'blocked', reason: null, text: 'Parked on a merge conflict' },
				'conflict_parked'
			),
			onaction: () => {},
			loadPlan: async () => null
		});
		await expect.element(page.getByText('Parked on a merge conflict')).toBeVisible();
		await expect.element(page.getByRole('link', { name: 'Open PR' })).toHaveAttribute('href', PR);
	});

	it('keeps Retry and adds Open PR when enzarb failed', async () => {
		const onaction = vi.fn();
		render(NeedsYouRow, {
			item: {
				...crRow({ category: 'failed', reason: 'boom' }, 'enzarb_failed'),
				state: 'Failed'
			},
			onaction,
			loadPlan: async () => null
		});
		await page.getByRole('button', { name: 'Retry' }).click();
		expect(onaction).toHaveBeenCalledWith('retry');
		await expect.element(page.getByRole('link', { name: 'Open PR' })).toHaveAttribute('href', PR);
	});

	it('links a decision to its comment', async () => {
		const comment = `${PR}#issuecomment-9`;
		render(NeedsYouRow, {
			item: crRow(
				{ category: 'decision', reason: null, text: 'Waiting on a decision' },
				'decision_needed',
				{
					url: comment
				}
			),
			onaction: () => {},
			loadPlan: async () => null
		});
		await expect
			.element(page.getByRole('link', { name: 'View comment' }))
			.toHaveAttribute('href', comment);
	});

	it('omits Review on GitHub when the pull request url is unknown', async () => {
		render(NeedsYouRow, {
			item: {
				...crRow(
					{ category: 'review', reason: null, text: 'Waiting for your review' },
					'awaiting_review'
				),
				url: null
			},
			onaction: () => {},
			loadPlan: async () => null
		});
		await expect.element(page.getByText('Waiting for your review')).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'Review on GitHub' }))
			.not.toBeInTheDocument();
	});
});
