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
			startedAt: null,
			finishedAt: null
		}
	],
	...over
});

describe('RunDetail', () => {
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
