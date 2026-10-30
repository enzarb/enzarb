import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { EventDecision, EventRow } from '#lib/events.ts';
import EventLog from './EventLog.svelte';

const row = (over: Partial<EventRow> = {}): EventRow => ({
	id: 1,
	event: 'issues',
	action: 'opened',
	repo: 'octo/app',
	sender: 'octocat',
	summary: 'Add search',
	receivedAt: '2026-10-21T12:00:00.000Z',
	...over
});

const base = { payload: async () => ({}), retry: () => {} };

describe('EventLog', () => {
	it('shows a skeleton while loading', async () => {
		render(EventLog, { ...base, rows: undefined });
		await expect.element(page.getByRole('status')).toBeVisible();
	});

	it('shows an empty state', async () => {
		render(EventLog, { ...base, rows: [] });
		await expect.element(page.getByText('No events yet')).toBeVisible();
	});

	it('offers a retry when loading failed', async () => {
		const retry = vi.fn();
		render(EventLog, { ...base, rows: undefined, failed: true, retry });
		await page.getByRole('button', { name: 'Retry' }).click();
		expect(retry).toHaveBeenCalledOnce();
	});

	it('lists events and loads the payload when a row opens', async () => {
		const payload = vi.fn(async () => ({ ref: 'refs/heads/main' }));
		render(EventLog, {
			...base,
			payload,
			rows: [row(), row({ id: 2, event: 'push', action: null, summary: null, sender: null })]
		});
		await expect.element(page.getByText('issues opened')).toBeVisible();
		await expect.element(page.getByText('Add search')).toBeVisible();
		expect(payload).not.toHaveBeenCalled();
		await page.getByText('push', { exact: true }).click();
		await expect.element(page.getByText(/refs\/heads\/main/)).toBeVisible();
		expect(payload).toHaveBeenCalledWith(2);
	});

	it('shows what tracks decided: the started workflow, or why no rule matched', async () => {
		const d = (over: Partial<EventDecision>): EventDecision => ({
			eventId: 1,
			outcome: 'no_match',
			project: 'p',
			sha: 'abc1234',
			started: [],
			reasons: [],
			errors: [],
			decidedAt: '2026-10-21T12:00:01.000Z',
			...over
		});
		render(EventLog, {
			...base,
			rows: [row(), row({ id: 2, event: 'push', action: null, summary: null })],
			decisions: {
				1: d({
					outcome: 'started',
					started: ['fix-issue'],
					reasons: [
						{
							workflow: 'fix-issue',
							matched: true,
							on: 'issues.opened',
							reason: 'on `issues.opened`'
						}
					]
				}),
				2: d({
					eventId: 2,
					reasons: [
						{
							workflow: 'fix-issue',
							matched: false,
							on: null,
							reason: '`push` is not in on = [issues.opened]'
						}
					]
				})
			}
		});
		await expect.element(page.getByText('Started fix-issue')).toBeVisible();
		await expect.element(page.getByText('No rule matched')).toBeVisible();
		await expect.element(page.getByText(/`push` is not in on/)).toBeVisible();
	});

	it('shows no decision for an event tracks has not decided yet', async () => {
		render(EventLog, { ...base, rows: [row()], decisions: {} });
		await expect.element(page.getByText('issues opened')).toBeVisible();
		await expect.element(page.getByTestId('event-decision')).not.toBeInTheDocument();
	});
});
