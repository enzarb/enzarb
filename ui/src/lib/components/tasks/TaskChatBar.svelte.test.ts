import { page } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { AcpChannel, AcpSession } from '#lib/acp/connection.ts';
import { fakeAgentChannel } from '#lib/test/acp-fake.ts';
import type { Task } from '#lib/tasks.ts';
import TaskChatBar from './TaskChatBar.svelte';

const chatConnection = vi.hoisted(() => vi.fn(() => new Promise(() => {})));
vi.mock('#lib/chat.remote.ts', () => ({ chatConnection }));

const review: Task = {
	kind: 'review',
	key: 'pr:acme/app#50',
	project: 'alpha',
	repo: 'acme/app',
	number: 50,
	title: 'Add retries',
	githubUrl: null,
	runUrl: null,
	at: '',
	branch: 'ez/implement/r9',
	draft: false
};
const triaged: Task = {
	kind: 'triaged',
	key: 'issue:acme/app#12',
	project: 'alpha',
	repo: 'acme/app',
	number: 12,
	title: 'Crash on start',
	githubUrl: null,
	runUrl: null,
	at: '',
	body: 'It crashes.'
};

function setup(features: string[]) {
	const ch = fakeAgentChannel({ sessionId: 's-1' });
	const openAcp = vi.fn(async () => ch as unknown as AcpChannel);
	const s: AcpSession = { features: new Set(features), openAcp };
	return { ch, openAcp, connect: vi.fn(async () => s) };
}

describe('TaskChatBar', () => {
	beforeEach(() => sessionStorage.clear());

	it('opens the chat on the task branch with the task context in the first message', async () => {
		const { ch, openAcp, connect } = setup(['acp', 'chat_checkout']);
		render(TaskChatBar, { task: review, connect });
		const box = page.getByRole('textbox', { name: 'Message' });
		await expect.element(box).toBeEnabled();
		expect(openAcp).toHaveBeenCalledWith({
			repo: 'https://github.com/acme/app.git',
			checkout: { branch: 'ez/implement/r9', start_ref: 'refs/heads/ez/implement/r9' }
		});
		await box.fill('please review');
		await page.getByRole('button', { name: 'Send' }).click();
		await vi.waitFor(() => {
			const p = ch.received.find((m) => m.method === 'session/prompt');
			const text = JSON.stringify(p?.params);
			expect(text).toContain('Add retries');
			expect(text).toContain('please review');
		});
	});

	it('says the sandbox needs an update without chat_checkout, and opens nothing', async () => {
		const { openAcp, connect } = setup(['acp']);
		render(TaskChatBar, { task: triaged, connect });
		await expect.element(page.getByText('Needs an update', { exact: true })).toBeVisible();
		expect(openAcp).not.toHaveBeenCalled();
	});

	it('switches chats with the task', async () => {
		const { openAcp, connect } = setup(['acp', 'chat_checkout']);
		const view = render(TaskChatBar, { task: review, connect });
		await expect.element(page.getByRole('textbox', { name: 'Message' })).toBeEnabled();
		await view.rerender({ task: triaged, connect });
		await vi.waitFor(() => expect(openAcp).toHaveBeenCalledTimes(2));
		expect(openAcp).toHaveBeenLastCalledWith({
			repo: 'https://github.com/acme/app.git',
			checkout: { branch: 'enzarb/issue-12' }
		});
	});
});
