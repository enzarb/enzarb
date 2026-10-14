import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Usage } from '#lib/usage.ts';
import type { NeedsYou, WorkItemSummary } from '#lib/work-items.ts';
import TaskCard from './TaskCard.svelte';

const NOW = Date.parse('2026-10-14T12:00:00Z');
const ago = (ms: number) => new Date(NOW - ms).toISOString();

const card = (
	needs: NeedsYou | null,
	state = 'AwaitingApproval',
	extra: Partial<WorkItemSummary> = {}
): WorkItemSummary => ({
	id: 7,
	key: 'github:acme/web#7',
	project: 'web',
	workflow: 'issue',
	state,
	title: 'Add search',
	url: 'https://github.com/acme/web/issues/7',
	updatedAt: ago(3 * 3_600_000),
	needs,
	...extra
});

const props = (item: WorkItemSummary, over = {}) => ({
	item,
	now: NOW,
	onaction: vi.fn(),
	...over
});

describe('TaskCard', () => {
	it('shows title, key, workflow, GitHub link, time since and the approve action', async () => {
		const onaction = vi.fn();
		render(TaskCard, props(card({ category: 'approval', reason: null }), { onaction }));
		await expect.element(page.getByText('Plan awaiting approval')).toBeVisible();
		await expect.element(page.getByText('github:acme/web#7')).toBeVisible();
		await expect.element(page.getByText('3h ago')).toBeVisible();
		const ref = page.getByRole('link', { name: 'acme/web#7' });
		await expect.element(ref).toHaveAttribute('href', 'https://github.com/acme/web/issues/7');
		await expect.element(ref).toHaveAttribute('target', '_blank');
		await expect.element(ref).toHaveAttribute('rel', expect.stringContaining('noopener'));
		const title = page.getByRole('link', { name: 'Add search' });
		await expect.element(title).toHaveAttribute('href', '/task/7');
		await expect.element(title).not.toHaveAttribute('target');
		await page.getByRole('button', { name: 'Approve plan' }).click();
		expect(onaction).toHaveBeenCalledWith('approve');
	});

	it('shows time since with the exact timestamp in the title', async () => {
		const item = card(null, 'Planning');
		render(TaskCard, props(item));
		const t = page.getByText('3h ago');
		await expect.element(t).toHaveAttribute('datetime', item.updatedAt);
		await expect.element(t).toHaveAttribute('title', item.updatedAt);
	});

	it('shows the project only when the page is not filtered to one', async () => {
		const item = card(null, 'Planning');
		const r = render(TaskCard, props(item));
		await expect.element(page.getByText('web', { exact: true })).toBeVisible();
		await r.rerender({ project: 'web' });
		await expect.element(page.getByText('web', { exact: true })).not.toBeInTheDocument();
	});

	it('has no state chip, and says why a task in progress waits', async () => {
		render(
			TaskCard,
			props(
				card(null, 'AwaitingChanges', {
					workflow: 'change_request',
					wait: { waitingOn: 'ci', reason: 'ci_pending', text: 'Waiting for CI', url: null }
				})
			)
		);
		await expect.element(page.getByText('Waiting for CI')).toBeVisible();
		await expect.element(page.getByText('AwaitingChanges')).not.toBeInTheDocument();
	});

	it('shows tokens and cost when the task reported usage', async () => {
		const usage: Usage = {
			inputTokens: 12_345,
			outputTokens: 1200,
			cacheReadTokens: 0,
			cacheWriteTokens: 0,
			models: [],
			costUsd: 0.42
		};
		render(TaskCard, props(card(null, 'Planning', { usage })));
		await expect.element(page.getByText('12.3k')).toBeVisible();
		await expect.element(page.getByText('1.2k')).toBeVisible();
		await expect.element(page.getByText('$0.42')).toBeVisible();
	});

	it('opens the plan from a plan approval', async () => {
		const onplan = vi.fn();
		render(TaskCard, props(card({ category: 'approval', reason: null }), { onplan }));
		await page.getByRole('button', { name: 'View plan' }).click();
		expect(onplan).toHaveBeenCalled();
	});

	it('opens the review from a change request waiting to merge', async () => {
		const onreview = vi.fn();
		render(
			TaskCard,
			props(
				card({ category: 'merge', reason: null }, 'ReadyToMerge', {
					workflow: 'change_request',
					url: 'https://github.com/acme/web/pull/7'
				}),
				{ onreview }
			)
		);
		await page.getByRole('button', { name: 'Review', exact: true }).click();
		expect(onreview).toHaveBeenCalled();
	});

	it('shows just #n for single-repo projects', async () => {
		render(
			TaskCard,
			props(card({ category: 'approval', reason: null }, 'AwaitingApproval', { singleRepo: true }))
		);
		await expect.element(page.getByRole('link', { name: '#7' })).toBeVisible();
	});

	it('falls back to "View on GitHub" for an unparseable URL', async () => {
		render(
			TaskCard,
			props(
				card({ category: 'approval', reason: null }, 'AwaitingApproval', {
					url: 'https://example.com/x'
				})
			)
		);
		await expect.element(page.getByRole('link', { name: 'View on GitHub' })).toBeVisible();
	});

	it('offers retry with the failure reason for failed items', async () => {
		const onaction = vi.fn();
		render(
			TaskCard,
			props(card({ category: 'failed', reason: 'job timed out' }, 'Failed'), { onaction })
		);
		await expect.element(page.getByText('job timed out')).toBeVisible();
		await page.getByRole('button', { name: 'Retry' }).click();
		expect(onaction).toHaveBeenCalledWith('retry');
	});

	it('asks for confirmation before merging', async () => {
		const onaction = vi.fn();
		render(
			TaskCard,
			props(card({ category: 'merge', reason: null }, 'ReadyToMerge'), { onaction })
		);
		await page.getByRole('button', { name: 'Merge (squash)' }).click();
		expect(onaction).not.toHaveBeenCalled();
		await page.getByRole('button', { name: 'Merge (squash)' }).last().click();
		expect(onaction).toHaveBeenCalledWith('merge');
	});

	it('shows the blocker and no action for blocked items', async () => {
		render(
			TaskCard,
			props(card({ category: 'blocked', reason: 'CI: tests red' }, 'AwaitingChanges'))
		);
		await expect.element(page.getByText('CI: tests red')).toBeVisible();
		await expect.element(page.getByRole('button')).not.toBeInTheDocument();
	});

	it('shows an action error', async () => {
		render(
			TaskCard,
			props(card({ category: 'failed', reason: null }, 'Failed'), {
				error: 'Could not retry this item. Its state may have changed.'
			})
		);
		await expect.element(page.getByRole('alert')).toHaveTextContent('Could not retry');
	});

	const PR = 'https://github.com/acme/web/pull/7';
	const crCard = (needs: NeedsYou, reason: string, extra = {}) =>
		card(needs, 'AwaitingChanges', {
			workflow: 'change_request',
			url: PR,
			wait: { waitingOn: 'human' as const, reason, text: needs.text ?? '', url: null, ...extra }
		});

	it('lists the unverified checks and links to the review for a change request', async () => {
		const text = 'enzarb could not verify: connectors tests (missing_tool)';
		render(
			TaskCard,
			props(crCard({ category: 'review', reason: null, text }, 'checks_unverified'))
		);
		await expect.element(page.getByText(text)).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'Review on GitHub' }))
			.toHaveAttribute('href', `${PR}/files`);
	});

	it('shows an unknown wait reason as its text without an action', async () => {
		render(
			TaskCard,
			props(crCard({ category: 'blocked', reason: null, text: 'Something new' }, 'from_the_future'))
		);
		await expect.element(page.getByText('Something new')).toBeVisible();
		await expect.element(page.getByRole('button')).not.toBeInTheDocument();
	});

	it('shows the reason text and a Review on GitHub button to the files tab', async () => {
		render(
			TaskCard,
			props(
				crCard(
					{ category: 'review', reason: null, text: 'Waiting for your review' },
					'awaiting_review'
				)
			)
		);
		await expect.element(page.getByText('Waiting for your review')).toBeVisible();
		await expect.element(page.getByText('Blocked')).not.toBeInTheDocument();
		await expect
			.element(page.getByRole('link', { name: 'Review on GitHub' }))
			.toHaveAttribute('href', `${PR}/files`);
	});

	it('offers Open PR for a parked conflict', async () => {
		render(
			TaskCard,
			props(
				crCard(
					{ category: 'blocked', reason: null, text: 'Parked on a merge conflict' },
					'conflict_parked'
				)
			)
		);
		await expect.element(page.getByText('Parked on a merge conflict')).toBeVisible();
		await expect.element(page.getByRole('link', { name: 'Open PR' })).toHaveAttribute('href', PR);
	});

	it('keeps Retry and adds Open PR when enzarb failed', async () => {
		const onaction = vi.fn();
		render(
			TaskCard,
			props(
				{ ...crCard({ category: 'failed', reason: 'boom' }, 'enzarb_failed'), state: 'Failed' },
				{ onaction }
			)
		);
		await page.getByRole('button', { name: 'Retry' }).click();
		expect(onaction).toHaveBeenCalledWith('retry');
		await expect.element(page.getByRole('link', { name: 'Open PR' })).toHaveAttribute('href', PR);
	});

	it('links a decision to its comment', async () => {
		const comment = `${PR}#issuecomment-9`;
		render(
			TaskCard,
			props(
				crCard(
					{ category: 'decision', reason: null, text: 'Waiting on a decision' },
					'decision_needed',
					{ url: comment }
				)
			)
		);
		await expect
			.element(page.getByRole('link', { name: 'View comment' }))
			.toHaveAttribute('href', comment);
	});

	it('omits Review on GitHub when the pull request url is unknown', async () => {
		render(
			TaskCard,
			props({
				...crCard(
					{ category: 'review', reason: null, text: 'Waiting for your review' },
					'awaiting_review'
				),
				url: null
			})
		);
		await expect.element(page.getByText('Waiting for your review')).toBeVisible();
		await expect
			.element(page.getByRole('link', { name: 'Review on GitHub' }))
			.not.toBeInTheDocument();
	});
});
