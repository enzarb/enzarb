import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Readiness, WorkItemDetail, WorkItemSummary } from '#lib/work-items.ts';
import ReviewDialog from './ReviewDialog.svelte';

const item: WorkItemSummary = {
	id: 7,
	key: 'github:acme/web#7',
	project: 'web',
	workflow: 'change_request',
	state: 'ReadyToMerge',
	title: 'Add search',
	url: 'https://github.com/acme/web/pull/7',
	updatedAt: '2026-10-14T09:00:00.000Z',
	needs: { category: 'merge', reason: null }
};

const ok = { status: 'ok' as const, detail: null };
const readiness = (over: Partial<Readiness> = {}): Readiness => ({
	ci: ok,
	review: ok,
	mergeable: ok,
	mergeError: null,
	...over
});

const detail = (over: Partial<WorkItemDetail> = {}): WorkItemDetail => ({
	...item,
	actions: ['merge', 'cancel'],
	transitions: [],
	jobs: [
		{
			id: 1,
			kind: 'agent_review',
			status: 'succeeded',
			attempt: 1,
			updatedAt: '2026-10-14T08:00:00.000Z',
			outputHtml: '<p>Looks good to me.</p>'
		}
	],
	readiness: readiness(),
	...over
});

const props = (over = {}) => ({
	open: true,
	item,
	load: vi.fn(async () => detail()),
	onmerge: vi.fn(async () => {}),
	...over
});

const dialog = () => page.getByRole('dialog', { name: /Review: Add search/ });
const mergeButton = () => dialog().getByRole('button', { name: 'Merge (squash)' });

describe('ReviewDialog', () => {
	it('shows the readiness, the latest review and the pull request link', async () => {
		const p = props();
		render(ReviewDialog, p);
		await expect.element(page.getByText('CI on head')).toBeVisible();
		await expect.element(page.getByText('enzarb review approved')).toBeVisible();
		await expect.element(page.getByText('Looks good to me.')).toBeVisible();
		const pr = dialog().getByRole('link', { name: 'acme/web#7' });
		await expect.element(pr).toHaveAttribute('href', 'https://github.com/acme/web/pull/7');
		await expect.element(pr).toHaveAttribute('target', '_blank');
		await expect.element(pr).toHaveAttribute('rel', expect.stringContaining('noopener'));
		expect(p.load).toHaveBeenCalledWith(7);
	});

	it('shows why the last merge did not happen', async () => {
		render(
			ReviewDialog,
			props({
				load: vi.fn(async () =>
					detail({ readiness: readiness({ mergeError: 'CI is not passing on the head' }) })
				)
			})
		);
		await expect.element(page.getByText('CI is not passing on the head')).toBeVisible();
	});

	it('offers the merge only while the change request can be merged', async () => {
		render(
			ReviewDialog,
			props({
				load: vi.fn(async () =>
					detail({
						state: 'AwaitingChanges',
						actions: ['cancel'],
						readiness: readiness({ ci: { status: 'pending', detail: null } })
					})
				)
			})
		);
		await expect
			.element(page.getByText('It can be merged once review and checks pass.'))
			.toBeVisible();
		await expect.element(mergeButton()).toBeDisabled();
		await expect
			.element(page.getByRole('checkbox', { name: 'Delete branch after merge' }))
			.toBeDisabled();
	});

	it('asks for confirmation, then merges and deletes the branch by default', async () => {
		const p = props();
		render(ReviewDialog, p);
		await expect.element(mergeButton()).toBeEnabled();
		await mergeButton().click();
		expect(p.onmerge).not.toHaveBeenCalled();
		await page.getByRole('button', { name: 'Merge (squash)' }).last().click();
		expect(p.onmerge).toHaveBeenCalledWith(7, { deleteBranch: true });
		await expect.element(page.getByText('Merge requested.')).toBeVisible();
	});

	it('passes on that the branch should be kept', async () => {
		const p = props();
		render(ReviewDialog, p);
		const box = page.getByRole('checkbox', { name: 'Delete branch after merge' });
		await expect.element(box).toBeChecked();
		await box.click();
		await expect.element(box).not.toBeChecked();
		await mergeButton().click();
		await page.getByRole('button', { name: 'Merge (squash)' }).last().click();
		expect(p.onmerge).toHaveBeenCalledWith(7, { deleteBranch: false });
	});

	it('shows an error when the merge cannot be requested', async () => {
		render(
			ReviewDialog,
			props({
				onmerge: vi.fn(async () => {
					throw new Error('conflict');
				})
			})
		);
		await mergeButton().click();
		await page.getByRole('button', { name: 'Merge (squash)' }).last().click();
		await expect.element(page.getByRole('alert')).toHaveTextContent('Could not merge');
		await expect.element(mergeButton()).toBeEnabled();
	});

	it('shows an error when the change request cannot be loaded', async () => {
		render(
			ReviewDialog,
			props({
				load: vi.fn(async () => {
					throw new Error('nope');
				})
			})
		);
		await expect.element(page.getByText('Could not load this change request.')).toBeVisible();
		await expect.element(mergeButton()).toBeDisabled();
	});

	it('reloads when the item changes while it is open', async () => {
		const load = vi.fn(async () => detail());
		const r = render(ReviewDialog, props({ load }));
		await expect.element(page.getByText('CI on head')).toBeVisible();
		expect(load).toHaveBeenCalledTimes(1);
		await r.rerender({ item: { ...item, updatedAt: '2026-10-14T09:05:00.000Z' } });
		await vi.waitFor(() => expect(load).toHaveBeenCalledTimes(2));
	});
});
