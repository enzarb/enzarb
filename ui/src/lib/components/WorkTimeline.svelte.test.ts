import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import WorkTimeline from './WorkTimeline.svelte';

const t = (id: number, from: string | null, to: string, eventType: string) => ({
	id,
	fromState: from,
	toState: to,
	eventType,
	at: '2026-10-08T12:00:00.000Z'
});

describe('WorkTimeline', () => {
	it('renders transitions as an ordered step list with the latest step current', async () => {
		render(WorkTimeline, {
			state: 'Planning',
			transitions: [
				t(1, null, 'Triaging', 'WorkItemOpened'),
				t(2, 'Triaging', 'Planning', 'JobSucceeded')
			]
		});
		const items = page.getByRole('listitem');
		await expect.element(items.nth(0)).toHaveAttribute('data-kind', 'done');
		await expect.element(items.nth(0)).toHaveTextContent('Triaging');
		await expect.element(items.nth(0)).toHaveTextContent('WorkItemOpened');
		await expect.element(items.nth(1)).toHaveAttribute('data-kind', 'current');
		await expect.element(items.nth(1)).toHaveTextContent('Triaging →');
		await expect.element(items.nth(1)).toHaveTextContent('Planning');
	});

	it('marks a Failed last step as failed and a Done last step as done', async () => {
		const failed = render(WorkTimeline, {
			state: 'Failed',
			transitions: [
				t(1, null, 'Triaging', 'WorkItemOpened'),
				t(2, 'Triaging', 'Failed', 'JobFailed')
			]
		});
		await expect.element(page.getByRole('listitem').nth(1)).toHaveAttribute('data-kind', 'failed');
		failed.unmount();
		render(WorkTimeline, {
			state: 'Done',
			transitions: [t(1, 'ChangeRequestOpened', 'Done', 'X')]
		});
		await expect.element(page.getByRole('listitem').nth(0)).toHaveAttribute('data-kind', 'done');
	});

	it('shows an explicit empty step instead of a spinner', async () => {
		render(WorkTimeline, { state: 'Triaging', transitions: [] });
		await expect
			.element(page.getByRole('listitem'))
			.toHaveTextContent('No transitions recorded yet');
	});
});
