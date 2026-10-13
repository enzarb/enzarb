import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { parseReadiness } from '#lib/work-items.ts';
import MergeReadiness from './MergeReadiness.svelte';

const data = (ci: string, review: string, mergeable: string, merge_error?: string) =>
	parseReadiness({
		checklist: {
			ci: { status: ci, detail: 'ci detail' },
			review: { status: review, detail: 'review detail' },
			mergeable: { status: mergeable, detail: 'merge detail' }
		},
		merge_error
	});

const states = (c: HTMLElement) =>
	[...c.querySelectorAll('li')].map((li) => (li as HTMLElement).dataset.state);

describe('MergeReadiness', () => {
	it('renders each status with its detail and no spinner', async () => {
		const { container } = render(MergeReadiness, {
			readiness: data('ok', 'pending', 'failed'),
			state: 'AwaitingChanges'
		});
		await expect.element(page.getByText('ci detail')).toBeVisible();
		await expect.element(page.getByText('review detail')).toBeVisible();
		await expect.element(page.getByText('merge detail')).toBeVisible();
		expect(states(container)).toEqual(['ok', 'pending', 'failed']);
		expect(container.querySelector('.ez-steps__item--active')).toBeNull();
	});
	it('shows unknown rows when readiness is missing', async () => {
		const { container } = render(MergeReadiness, { readiness: null, state: 'ReadyToMerge' });
		expect(states(container)).toEqual(['unknown', 'unknown', 'unknown']);
		await expect.element(page.getByText('Unknown').first()).toBeVisible();
		expect(container.querySelector('.ez-steps__item--active')).toBeNull();
	});
	it('shows the merge steps while Merging', async () => {
		render(MergeReadiness, { readiness: data('ok', 'ok', 'ok'), state: 'Merging' });
		await expect.element(page.getByText('Re-checking readiness')).toBeVisible();
		await expect.element(page.getByText('Squash-merging')).toBeVisible();
		await expect.element(page.getByText('Done')).toBeVisible();
	});
	it('shows merge_error as a notice only when non-empty', async () => {
		const r = render(MergeReadiness, {
			readiness: data('ok', 'ok', 'ok', 'head moved'),
			state: 'Reviewing'
		});
		await expect.element(page.getByText('head moved')).toBeVisible();
		expect(r.container.querySelector('li')).toBeNull();
		r.unmount();
		const e = render(MergeReadiness, { readiness: data('ok', 'ok', 'ok', ''), state: 'Reviewing' });
		expect(e.container.querySelector('.ez-notice')).toBeNull();
	});
});
