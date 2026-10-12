import { page } from 'vitest/browser';
import { describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { AcpChannel, AcpSession } from '#lib/acp/connection.ts';
import { fakeAgentChannel, type FakeChannel, type Msg } from '#lib/test/acp-fake.ts';
import Chat from './Chat.svelte';

const terminalConnection = vi.hoisted(() => vi.fn(() => new Promise(() => {})));
vi.mock('#lib/terminal.remote.ts', () => ({ terminalConnection }));

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

describe('Chat', () => {
	it('shows "not available" and never opens acp on an older sandbox/proxy', async () => {
		const { openAcp, connect } = session([]);
		render(Chat, { project: 'alpha', connect });
		await expect.element(page.getByText('Chat not available')).toBeVisible();
		expect(openAcp).not.toHaveBeenCalled();
		expect(terminalConnection).not.toHaveBeenCalled();
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
