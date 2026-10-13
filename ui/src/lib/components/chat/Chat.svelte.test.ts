import { page } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { AcpChannel, AcpSession } from '#lib/acp/connection.ts';
import { fakeAgentChannel, type FakeChannel, type Msg } from '#lib/test/acp-fake.ts';
import Chat from './Chat.svelte';

const chatConnection = vi.hoisted(() => vi.fn(() => new Promise(() => {})));
vi.mock('#lib/chat.remote.ts', () => ({ chatConnection }));

function session(features: string[], ch?: FakeChannel) {
	const openAcp = vi.fn(async () => ch as unknown as AcpChannel);
	const s: AcpSession = { features: new Set(features), openAcp };
	const connect = vi.fn(async (report: (s: 'token' | 'connect' | 'handshake') => void) => {
		report('token');
		report('connect');
		report('handshake');
		return s;
	});
	return { openAcp, connect };
}

function multi(...chs: FakeChannel[]) {
	let n = 0;
	const openAcp = vi.fn(async () => chs[Math.min(n++, chs.length - 1)] as unknown as AcpChannel);
	const s: AcpSession = { features: new Set(['acp']), openAcp };
	const connect = vi.fn(async () => s);
	return { openAcp, connect };
}

describe('Chat resume', () => {
	beforeEach(() => sessionStorage.clear());

	it('offers the saved id as open.resume after a reload', async () => {
		sessionStorage.setItem('enzarb.chat.alpha', 'saved-1');
		const ch = fakeAgentChannel({ sessionId: 'saved-1', loadSession: true });
		const { openAcp, connect } = multi(ch);
		render(Chat, { project: 'alpha', connect });
		await expect.element(page.getByRole('textbox', { name: 'Message' })).toBeEnabled();
		expect(openAcp).toHaveBeenCalledWith({ resume: 'saved-1' });
		expect(ch.received.some((m) => m.method === 'session/load')).toBe(true);
		expect(sessionStorage.getItem('enzarb.chat.alpha')).toBe('saved-1');
	});

	it('stores the id of a fresh chat', async () => {
		const ch = fakeAgentChannel({ sessionId: 'fresh-1' });
		const { connect } = multi(ch);
		render(Chat, { project: 'alpha', connect });
		await expect.element(page.getByRole('textbox', { name: 'Message' })).toBeEnabled();
		expect(sessionStorage.getItem('enzarb.chat.alpha')).toBe('fresh-1');
	});

	it('an unknown id gives a fresh chat, the notice, and replaces the stored id', async () => {
		sessionStorage.setItem('enzarb.chat.alpha', 'gone');
		const ch = fakeAgentChannel({
			sessionId: 'new-1',
			loadSession: true,
			load: () => new Error('unknown session')
		});
		const { connect } = multi(ch);
		render(Chat, { project: 'alpha', connect });
		await expect.element(page.getByText(/could not be restored/)).toBeVisible();
		expect(sessionStorage.getItem('enzarb.chat.alpha')).toBe('new-1');
	});

	it('New chat clears the stored id and opens without resume', async () => {
		sessionStorage.setItem('enzarb.chat.alpha', 'saved-1');
		const first = fakeAgentChannel({ sessionId: 'saved-1', loadSession: true });
		const second = fakeAgentChannel({ sessionId: 'second-1', loadSession: true });
		const { openAcp, connect } = multi(first, second);
		render(Chat, { project: 'alpha', connect });
		await expect.element(page.getByRole('textbox', { name: 'Message' })).toBeEnabled();
		await page.getByRole('button', { name: 'New chat' }).click();
		await vi.waitFor(() => expect(openAcp).toHaveBeenCalledTimes(2));
		expect(openAcp).toHaveBeenLastCalledWith({});
		expect(second.received.some((m) => m.method === 'session/new')).toBe(true);
		await vi.waitFor(() => expect(sessionStorage.getItem('enzarb.chat.alpha')).toBe('second-1'));
	});

	it('still works when sessionStorage throws', async () => {
		const boom = () => {
			throw new Error('denied');
		};
		vi.spyOn(Storage.prototype, 'getItem').mockImplementation(boom);
		vi.spyOn(Storage.prototype, 'setItem').mockImplementation(boom);
		vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(boom);
		try {
			const ch = fakeAgentChannel({ sessionId: 's' });
			const { openAcp, connect } = multi(ch);
			render(Chat, { project: 'alpha', connect });
			await expect.element(page.getByRole('textbox', { name: 'Message' })).toBeEnabled();
			expect(openAcp).toHaveBeenCalledWith({});
		} finally {
			vi.restoreAllMocks();
		}
	});
});

describe('Chat', () => {
	it('shows "not available" and never opens acp on an older sandbox/proxy', async () => {
		const { openAcp, connect } = session([]);
		render(Chat, { project: 'alpha', connect });
		await expect.element(page.getByText('Chat not available')).toBeVisible();
		expect(openAcp).not.toHaveBeenCalled();
		expect(chatConnection).not.toHaveBeenCalled();
	});

	it('prompt -> streamed update -> permission -> Allow once', async () => {
		let prompt: Msg | undefined;
		const ch = fakeAgentChannel({
			sessionId: 'sess-1',
			onMessage(m, c) {
				if (m.method === 'session/prompt') {
					prompt = m;
					c.send({
						jsonrpc: '2.0',
						method: 'session/update',
						params: {
							sessionId: 'sess-1',
							update: {
								sessionUpdate: 'agent_message_chunk',
								content: { type: 'text', text: 'Looking at the repo' }
							}
						}
					});
					c.send({
						jsonrpc: '2.0',
						id: 900,
						method: 'session/request_permission',
						params: {
							sessionId: 'sess-1',
							toolCall: { toolCallId: 't1', title: 'Run npm test' },
							options: [
								{ optionId: 'ok', name: 'Allow', kind: 'allow_once' },
								{ optionId: 'no', name: 'Reject', kind: 'reject_once' }
							]
						}
					});
					return true; // reply to the prompt after the permission answer
				}
				if (m.id === 900 && 'result' in m && prompt) {
					c.send({ jsonrpc: '2.0', id: prompt.id, result: { stopReason: 'end_turn' } });
				}
			}
		});
		const { openAcp, connect } = session(['acp'], ch);
		render(Chat, { project: 'alpha', connect });

		const box = page.getByRole('textbox', { name: 'Message' });
		await expect.element(box).toBeEnabled();
		await box.fill('run the tests');
		await page.getByRole('button', { name: 'Send' }).click();

		await expect.element(page.getByText('run the tests')).toBeVisible();
		await expect.element(page.getByText('Looking at the repo')).toBeVisible();
		await expect.element(page.getByText('Run npm test')).toBeVisible();
		await page.getByRole('button', { name: 'Allow once' }).click();

		await expect
			.poll(() => ch.received.find((m) => m.id === 900 && 'result' in m)?.result)
			.toEqual({ outcome: { outcome: 'selected', optionId: 'ok' } });
		expect(openAcp).toHaveBeenCalledTimes(1);
		const newSession = ch.received.find((m) => m.method === 'session/new');
		expect(newSession).toBeDefined();
	});
});

describe('Chat permission mode', () => {
	beforeEach(() => sessionStorage.clear());

	it('shows nothing for an old sandbox and the badge after a mode notification', async () => {
		const ch = fakeAgentChannel({ sessionId: 's-1' });
		const { connect } = multi(ch);
		render(Chat, { project: 'alpha', connect });
		await expect.element(page.getByRole('textbox', { name: 'Message' })).toBeEnabled();
		expect(document.querySelector('.ez-chat-mode')).toBeNull();

		ch.send({
			jsonrpc: '2.0',
			method: '_enzarb/mode',
			params: { sessionId: 's-1', modeId: 'bypassPermissions', bypassAllowed: true }
		});
		await expect.element(page.getByText('Bypass – no permission prompts')).toBeVisible();
		await expect.element(page.getByRole('combobox', { name: 'Permission mode' })).toBeEnabled();
	});
});
