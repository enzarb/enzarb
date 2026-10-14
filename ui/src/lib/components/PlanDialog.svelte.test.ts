import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { WorkItemSummary } from '#lib/work-items.ts';
import PlanDialog from './PlanDialog.svelte';

const item: WorkItemSummary = {
	id: 7,
	key: 'github:acme/web#7',
	project: 'web',
	workflow: 'issue',
	state: 'AwaitingApproval',
	title: 'Add search',
	url: 'https://github.com/acme/web/issues/7',
	updatedAt: '2026-10-14T09:00:00.000Z',
	needs: { category: 'approval', reason: null }
};

const props = (over = {}) => ({
	open: true,
	item,
	load: vi.fn(async () => '<h2>The plan</h2><p>Add a search box.</p>'),
	onaction: vi.fn(async () => {}),
	oncomment: vi.fn(async () => {}),
	...over
});

const dialog = () => page.getByRole('dialog', { name: /Plan: Add search/ });

describe('PlanDialog', () => {
	it('loads the plan for the item and renders it', async () => {
		const p = props();
		render(PlanDialog, p);
		await expect.element(dialog()).toBeVisible();
		await expect.element(page.getByRole('heading', { name: 'The plan' })).toBeVisible();
		await expect.element(page.getByText('Add a search box.')).toBeVisible();
		expect(p.load).toHaveBeenCalledWith(7);
	});

	it('says so when there is no plan yet', async () => {
		render(PlanDialog, props({ load: vi.fn(async () => null) }));
		await expect.element(page.getByText('No plan yet.')).toBeVisible();
	});

	it('shows an error when the plan cannot be loaded', async () => {
		render(
			PlanDialog,
			props({
				load: vi.fn(async () => {
					throw new Error('nope');
				})
			})
		);
		await expect.element(page.getByText('Could not load the plan.')).toBeVisible();
	});

	it('needs feedback text before it requests a revision', async () => {
		render(PlanDialog, props());
		const send = page.getByRole('button', { name: 'Request revision' });
		await expect.element(send).toBeDisabled();
		await page.getByLabelText('Feedback on the plan').fill('   ');
		await expect.element(send).toBeDisabled();
		await page.getByLabelText('Feedback on the plan').fill('use CSS vars');
		await expect.element(send).toBeEnabled();
	});

	it('sends the feedback and closes', async () => {
		const p = props();
		render(PlanDialog, p);
		await page.getByLabelText('Feedback on the plan').fill('  use CSS vars ');
		await page.getByRole('button', { name: 'Request revision' }).click();
		expect(p.oncomment).toHaveBeenCalledWith(7, 'use CSS vars');
		await expect.element(dialog()).not.toBeInTheDocument();
	});

	it('keeps the dialog and the text when sending fails', async () => {
		render(
			PlanDialog,
			props({
				oncomment: vi.fn(async () => {
					throw new Error('conflict');
				})
			})
		);
		await page.getByLabelText('Feedback on the plan').fill('use CSS vars');
		await page.getByRole('button', { name: 'Request revision' }).click();
		await expect.element(page.getByRole('alert')).toHaveTextContent('Could not send your feedback');
		await expect.element(page.getByLabelText('Feedback on the plan')).toHaveValue('use CSS vars');
	});

	it('approves from the dialog and closes', async () => {
		const p = props();
		render(PlanDialog, p);
		await page.getByRole('button', { name: 'Approve plan' }).click();
		expect(p.onaction).toHaveBeenCalledWith(7, 'approve');
		await expect.element(dialog()).not.toBeInTheDocument();
	});

	it('can cancel the task, and reports a failed action', async () => {
		const p = props({
			onaction: vi.fn(async () => {
				throw new Error('gone');
			})
		});
		render(PlanDialog, p);
		await page.getByRole('button', { name: 'Cancel' }).click();
		expect(p.onaction).toHaveBeenCalledWith(7, 'cancel');
		await expect.element(page.getByRole('alert')).toHaveTextContent('Could not cancel');
		await expect.element(dialog()).toBeVisible();
	});

	it('closes without doing anything', async () => {
		const p = props();
		render(PlanDialog, p);
		await page.getByRole('button', { name: 'Close' }).click();
		await expect.element(dialog()).not.toBeInTheDocument();
		expect(p.onaction).not.toHaveBeenCalled();
		expect(p.oncomment).not.toHaveBeenCalled();
	});
});
