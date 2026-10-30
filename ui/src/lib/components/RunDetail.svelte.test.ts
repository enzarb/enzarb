import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { RunDetail as Run } from '#lib/runs.ts';
import RunDetail from './RunDetail.svelte';

const run = (over: Partial<Run> = {}): Run => ({
	id: '11111111-1111-4111-8111-111111111111',
	workflow: 'wf',
	project: 'p',
	repo: 'o/r',
	subject: { kind: 'issue', number: 7 },
	eventId: 1,
	sha: 'abc1234def',
	status: 'running',
	test: false,
	currentNode: 'a',
	reason: null,
	stopRequested: false,
	createdAt: '2026-10-30T12:00:00.000Z',
	updatedAt: '2026-10-30T12:00:00.000Z',
	finishedAt: null,
	waitUntil: null,
	nodeRuns: [
		{
			node: 'a',
			visit: 2,
			status: 'failed',
			rendered: { type: 'agent', mode: 'triage', instruction: 'Do it' },
			mode: 'triage',
			outputs: { result: { ok: true } },
			usage: { input_tokens: 3, output_tokens: 4 },
			git: [{ repo: 'o/r', branch: 'ez/wf/x', head_sha: 'deadbeef', pushed: false }],
			error: 'boom',
			intent: null,
			wait: null,
			startedAt: null,
			finishedAt: null
		}
	],
	...over
});

describe('RunDetail', () => {
	const extra = (nodeRuns: Run['nodeRuns']) => run({ nodeRuns });
	const base = {
		visit: 1,
		mode: null,
		usage: null,
		git: null,
		error: null,
		intent: null,
		startedAt: null,
		finishedAt: null
	};

	it('shows what an action did, with its link', async () => {
		render(RunDetail, {
			stop: async () => {},
			run: extra([
				{
					...base,
					node: 'open_pr',
					status: 'done',
					rendered: {
						type: 'action',
						action: 'open_pull_request',
						title: 'Fix',
						body: 'Closes #7'
					},
					outputs: { number: 42, url: 'https://github.com/o/r/pull/42' },
					wait: null
				}
			])
		});
		const node = page.getByRole('listitem', { name: 'Node open_pr' });
		await expect
			.element(node.getByRole('link', { name: 'Opened pull request #42' }))
			.toHaveAttribute('href', 'https://github.com/o/r/pull/42');
		await expect.element(node).toHaveTextContent('Closes #7');
	});

	it('marks a test run action as not executed and unlinked', async () => {
		render(RunDetail, {
			stop: async () => {},
			run: extra([
				{
					...base,
					node: 'say',
					status: 'done',
					rendered: { type: 'action', action: 'comment', body: 'hi' },
					outputs: { number: 9000001, url: 'https://github.com/o/r/test-run/x', test: true },
					wait: null
				}
			])
		});
		const node = page.getByRole('listitem', { name: 'Node say' });
		await expect.element(node).toHaveTextContent('Would have: Commented');
		await expect.element(node.getByRole('link')).not.toBeInTheDocument();
	});

	it('shows what a waiting node waits for and until when', async () => {
		render(RunDetail, {
			stop: async () => {},
			run: extra([
				{
					...base,
					node: 'w',
					status: 'waiting',
					rendered: { type: 'wait', on: ['issue_comment.created'] },
					outputs: null,
					wait: {
						on: ['issue_comment.created'],
						if: 'event.x',
						timeout_secs: 60,
						until: '2026-11-06T12:00:00.000Z'
					}
				}
			])
		});
		const w = page.getByTestId('node-wait');
		await expect
			.element(w)
			.toHaveTextContent('Waiting for issue_comment.created if event.x, until');
	});

	it('shows how a finished wait ended', async () => {
		render(RunDetail, {
			stop: async () => {},
			run: extra([
				{
					...base,
					node: 'w',
					status: 'done',
					rendered: { type: 'wait', on: ['issue_comment.created'] },
					outputs: { timed_out: true, event: null },
					wait: {
						on: ['issue_comment.created'],
						if: null,
						timeout_secs: 1,
						until: null,
						timed_out: true
					}
				}
			])
		});
		await expect
			.element(page.getByTestId('node-wait'))
			.toHaveTextContent('Timed out waiting for issue_comment.created');
	});

	it('shows the node: instruction, mode, usage, branch and error', async () => {
		render(RunDetail, { run: run(), stop: async () => {} });
		const node = page.getByRole('listitem', { name: 'Node a' });
		await expect.element(node).toHaveTextContent('Do it');
		await expect.element(node).toHaveTextContent('visit 2');
		await expect.element(node).toHaveTextContent('mode triage');
		await expect.element(node).toHaveTextContent('3 in, 4 out');
		await expect.element(node).toHaveTextContent('not pushed');
		await expect.element(page.getByRole('alert')).toHaveTextContent('boom');
	});

	it('confirms before stopping a live run', async () => {
		const stop = vi.fn(async () => {});
		render(RunDetail, { run: run(), stop });
		await page.getByRole('button', { name: 'Stop', exact: true }).click();
		expect(stop).not.toHaveBeenCalled();
		await page.getByRole('button', { name: 'Stop run' }).click();
		await vi.waitFor(() => expect(stop).toHaveBeenCalledOnce());
	});

	it('has no Stop button once the run ended, and shows the reason', async () => {
		render(RunDetail, {
			run: run({ status: 'failed', reason: 'no_credential' }),
			stop: async () => {}
		});
		await expect
			.element(page.getByRole('button', { name: 'Stop', exact: true }))
			.not.toBeInTheDocument();
		await expect.element(page.getByText('no_credential')).toBeVisible();
	});
});
