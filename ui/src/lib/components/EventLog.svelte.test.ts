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
			resumed: [],
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

	it('shows which run an event resumed', async () => {
		render(EventLog, {
			...base,
			rows: [row()],
			decisions: {
				1: {
					eventId: 1,
					outcome: 'resumed',
					project: 'p',
					sha: 'abc1234',
					started: [],
					reasons: [],
					errors: [],
					resumed: [{ run: '11111111-aaaa-4bbb-8ccc-222222222222', workflow: 'fix', node: 'w' }],
					decidedAt: '2026-10-21T12:00:01.000Z'
				}
			}
		});
		await expect.element(page.getByText('Resumed run 11111111 (fix, w)')).toBeVisible();
	});

	it('can show only events that started or resumed a run', async () => {
		const d = (eventId: number, outcome: EventDecision['outcome']): EventDecision => ({
			eventId,
			outcome,
			project: 'p',
			sha: 'abc1234',
			started: [],
			reasons: [],
			errors: [],
			resumed: [],
			decidedAt: '2026-10-21T12:00:01.000Z'
		});
		render(EventLog, {
			...base,
			rows: [
				row({ id: 1, event: 'push', action: null }),
				row({ id: 2, event: 'issues', action: 'opened' }),
				row({ id: 3, event: 'issue_comment', action: 'created' }),
				row({ id: 4, event: 'pull_request', action: 'closed' })
			],
			decisions: { 1: d(1, 'no_match'), 2: d(2, 'started'), 3: d(3, 'resumed') }
		});
		await expect.element(page.getByText('push', { exact: true })).toBeVisible();
		await page.getByRole('checkbox', { name: 'Only runs started or resumed' }).click();
		await expect.element(page.getByText('push', { exact: true })).not.toBeInTheDocument();
		await expect.element(page.getByText('pull_request closed')).not.toBeInTheDocument();
		await expect.element(page.getByText('issues opened')).toBeVisible();
		await expect.element(page.getByText('issue_comment created')).toBeVisible();
	});

	it('says so when the filter hides every event', async () => {
		render(EventLog, { ...base, rows: [row()], decisions: {} });
		await page.getByRole('checkbox', { name: 'Only runs started or resumed' }).click();
		await expect.element(page.getByText('No runs started or resumed')).toBeVisible();
	});

	it('shows no decision for an event tracks has not decided yet', async () => {
		render(EventLog, { ...base, rows: [row()], decisions: {} });
		await expect.element(page.getByText('issues opened')).toBeVisible();
		await expect.element(page.getByTestId('event-decision')).not.toBeInTheDocument();
	});
});
