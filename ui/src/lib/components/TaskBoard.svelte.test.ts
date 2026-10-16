import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { NeedsYou, WorkItemSummary } from '#lib/work-items.ts';
import TaskBoard from './TaskBoard.svelte';

const NOW = Date.parse('2026-10-14T12:00:00Z');

const item = (
	id: number,
	state: string,
	needs: NeedsYou | null = null,
	extra: Partial<WorkItemSummary> = {}
): WorkItemSummary => ({
	id,
	key: `github:acme/web#${id}`,
	project: 'web',
	workflow: 'issue',
	state,
	title: `Task ${id}`,
	url: null,
	updatedAt: new Date(NOW - id * 60_000).toISOString(),
	needs,
	...extra
});

const props = (items: WorkItemSummary[], over = {}) => ({
	items,
	now: NOW,
	onaction: vi.fn(async () => {}),
	...over
});

const region = (name: string) => page.getByRole('region', { name });

describe('TaskBoard', () => {
	it('lists the columns in process order', async () => {
		render(TaskBoard, props([item(1, 'Planning')]));
		const titles = page
			.getByRole('heading', { level: 3 })
			.elements()
			.map((h) => (h.textContent ?? '').trim());
		expect(titles).toHaveLength(5);
		const order = ['Plan approval', 'In progress', 'Blocked / failed', 'Review & merge', 'Done'];
		order.forEach((t, i) => expect(titles[i].startsWith(t)).toBe(true));
	});

	it('puts every task in the column its state decides', async () => {
		render(
			TaskBoard,
			props([
				item(1, 'AwaitingApproval', { category: 'approval', reason: null }),
				item(2, 'Planning'),
				item(3, 'Failed', { category: 'failed', reason: 'boom' }),
				item(
					4,
					'ReadyToMerge',
					{ category: 'merge', reason: null },
					{ workflow: 'change_request' }
				),
				item(5, 'Done'),
				item(6, 'Cancelled')
			])
		);
		await expect.element(region('Plan approval').getByText('Task 1')).toBeVisible();
		await expect.element(region('In progress').getByText('Task 2')).toBeVisible();
		await expect.element(region('Blocked / failed').getByText('Task 3')).toBeVisible();
		await expect.element(region('Review & merge').getByText('Task 4')).toBeVisible();
		await expect.element(region('Done').getByText('Task 5')).toBeVisible();
		await expect.element(region('Done').getByText('Task 6')).toBeVisible();
	});

	it('shows a task waiting on the user once, in a needs-you column', async () => {
		render(TaskBoard, props([item(1, 'AwaitingApproval', { category: 'approval', reason: null })]));
		expect(page.getByRole('link', { name: 'Task 1' }).elements()).toHaveLength(1);
		await expect.element(region('Plan approval')).toHaveClass('ez-board__col--needs');
		await expect.element(region('In progress')).not.toHaveClass('ez-board__col--needs');
	});

	it('keeps empty columns, with a placeholder and a count', async () => {
		render(TaskBoard, props([item(1, 'Planning')]));
		await expect.element(region('Plan approval').getByText('Nothing here')).toBeVisible();
		await expect.element(region('Review & merge').getByText('Nothing here')).toBeVisible();
		await expect.element(region('In progress')).toHaveTextContent('1');
	});

	it('shows the project on cards unless the page is filtered to one', async () => {
		const r = render(TaskBoard, props([item(1, 'Planning')]));
		await expect.element(page.getByText('web', { exact: true })).toBeVisible();
		await r.rerender({ project: 'web' });
		await expect.element(page.getByText('web', { exact: true })).not.toBeInTheDocument();
	});

	it('has no State column or chip', async () => {
		render(TaskBoard, props([item(1, 'Planning'), item(2, 'Done')]));
		await expect.element(page.getByRole('columnheader')).not.toBeInTheDocument();
		await expect.element(page.getByText('Planning')).not.toBeInTheDocument();
	});

	it('moves a card when a live update changes its state', async () => {
		const r = render(TaskBoard, props([item(1, 'Planning')]));
		await expect.element(region('In progress').getByText('Task 1')).toBeVisible();
		await r.rerender({
			items: [item(1, 'AwaitingApproval', { category: 'approval', reason: null })]
		});
		await expect.element(region('Plan approval').getByText('Task 1')).toBeVisible();
		await expect.element(region('In progress').getByText('Task 1')).not.toBeInTheDocument();
		await r.rerender({ items: [item(1, 'Done')] });
		await expect.element(region('Done').getByText('Task 1')).toBeVisible();
	});

	it('caps Done at 20 and lists the rest on request', async () => {
		const done = Array.from({ length: 25 }, (_, i) => item(i + 1, 'Done'));
		render(TaskBoard, props(done));
		const links = () =>
			region('Done')
				.getByRole('link', { name: /^Task / })
				.elements().length;
		expect(links()).toBe(20);
		await page.getByRole('button', { name: 'Show all (25)' }).click();
		expect(links()).toBe(25);
		await page.getByRole('button', { name: 'Show fewer' }).click();
		expect(links()).toBe(20);
	});

	it('does not offer Show all for a short Done column', async () => {
		render(TaskBoard, props([item(1, 'Done')]));
		await expect.element(page.getByRole('button', { name: /Show all/ })).not.toBeInTheDocument();
	});

	it('shows a loading state and an empty state', async () => {
		const r = render(TaskBoard, props([], { loading: true }));
		await expect.element(page.getByRole('status')).toHaveTextContent('Loading tasks');
		await expect.element(page.getByRole('region', { name: 'Done' })).not.toBeInTheDocument();
		await r.rerender({ loading: false });
		await expect.element(page.getByText('No tasks yet')).toBeVisible();
	});

	it('approves inline and confirms before merging', async () => {
		const onaction = vi.fn(async () => {});
		render(
			TaskBoard,
			props(
				[
					item(1, 'AwaitingApproval', { category: 'approval', reason: null }),
					item(2, 'ReadyToMerge', { category: 'merge', reason: null })
				],
				{ onaction }
			)
		);
		await page.getByRole('button', { name: 'Approve plan' }).click();
		expect(onaction).toHaveBeenCalledWith(1, 'approve');
		await page.getByRole('button', { name: 'Merge (squash)' }).click();
		expect(onaction).toHaveBeenCalledTimes(1);
		await page.getByRole('button', { name: 'Merge (squash)' }).last().click();
		expect(onaction).toHaveBeenCalledWith(2, 'merge');
	});

	it('shows the error when an action fails', async () => {
		const onaction = vi.fn(async () => {
			throw new Error('conflict');
		});
		render(
			TaskBoard,
			props([item(1, 'Failed', { category: 'failed', reason: null })], { onaction })
		);
		await page.getByRole('button', { name: 'Retry' }).click();
		await expect.element(page.getByRole('alert')).toHaveTextContent('Could not retry');
	});

	describe('highlight from a notification', () => {
		const review: NeedsYou = { category: 'review', reason: null };
		const card = (name: string) =>
			page.getByRole('link', { name }).element().closest('li') as HTMLElement;

		it('scrolls to, focuses and accents the card, with no note when unchanged', async () => {
			const spy = vi.spyOn(HTMLElement.prototype, 'scrollIntoView');
			render(
				TaskBoard,
				props([item(1, 'Planning'), item(2, 'Open', review, { workflow: 'change_request' })], {
					highlight: { id: 2, was: 'review' }
				})
			);
			await expect.element(region('Review & merge').getByText('Task 2')).toBeVisible();
			await vi.waitFor(() => expect(document.activeElement).toBe(card('Task 2')));
			expect(card('Task 2')).toHaveClass('ez-card--highlight');
			expect(card('Task 1')).not.toHaveClass('ez-card--highlight');
			expect(spy).toHaveBeenCalled();
			await expect.element(page.getByText(/^Was /)).not.toBeInTheDocument();
			spy.mockRestore();
		});

		it('notes what changed when the item moved on', async () => {
			render(
				TaskBoard,
				props(
					[
						item(2, 'ResolvingConflicts', null, {
							workflow: 'change_request',
							wait: { waitingOn: 'enzarb', reason: 'x', text: 'Resolving conflicts', url: null }
						})
					],
					{ highlight: { id: 2, was: 'review' } }
				)
			);
			await expect
				.element(
					region('In progress').getByText('Was waiting for your review — now: Resolving conflicts')
				)
				.toBeVisible();
			await vi.waitFor(() => expect(document.activeElement).toBe(card('Task 2')));
		});

		it('lists a highlighted card past the Done cap', async () => {
			const done = Array.from({ length: 25 }, (_, i) => item(i + 1, 'Done'));
			render(TaskBoard, props(done, { highlight: { id: 25, was: 'merge' } }));
			await expect.element(page.getByText('Was ready to merge — now: done')).toBeVisible();
		});

		it('shows a banner linking to the task when it is not on the board', async () => {
			render(TaskBoard, props([item(1, 'Planning')], { highlight: { id: 9, was: 'failed' } }));
			await expect.element(page.getByText('This task is not on the board')).toBeVisible();
			await expect
				.element(page.getByRole('link', { name: 'Open task #9' }))
				.toHaveAttribute('href', '/task/9');
		});
	});

	it('opens the plan and review dialogs from their cards', async () => {
		const onplan = vi.fn();
		const onreview = vi.fn();
		const plan = item(1, 'AwaitingApproval', { category: 'approval', reason: null });
		const cr = item(
			2,
			'ReadyToMerge',
			{ category: 'merge', reason: null },
			{ workflow: 'change_request' }
		);
		render(TaskBoard, props([plan, cr], { onplan, onreview }));
		await page.getByRole('button', { name: 'View plan' }).click();
		expect(onplan).toHaveBeenCalledWith(plan);
		await page.getByRole('button', { name: 'Review', exact: true }).click();
		expect(onreview).toHaveBeenCalledWith(cr);
	});
});
