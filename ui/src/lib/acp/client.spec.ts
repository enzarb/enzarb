import { describe, expect, it, vi } from 'vitest';
import { AcpLimitError, createAcpClient, type AcpHandlers } from './client.ts';
import { MAX_PENDING, MAX_PROMPT_BYTES } from './types.ts';
import { fakeAgentChannel, type AgentOpts } from '../test/acp-fake.ts';

function setup(agent: AgentOpts = {}, over: Partial<AcpHandlers> = {}) {
	const ch = fakeAgentChannel(agent);
	const handlers: AcpHandlers = {
		sessionUpdate: vi.fn(),
		requestPermission: vi.fn(async () => ({ outcome: 'cancelled' as const })),
		permissionDenied: vi.fn(),
		...over
	};
	return { ch, handlers, client: createAcpClient(ch, handlers) };
}

const tick = () => new Promise((r) => setTimeout(r, 10));

describe('outbound surface', () => {
	it('advertises no fs or terminal capabilities and never sends _meta or authenticate', async () => {
		const { ch, client } = setup();
		await client.initialize();
		await client.newSession();
		await client.prompt('sess-new', 'hi');
		await client.cancel('sess-new');
		await tick();
		const init = ch.received.find((m) => m.method === 'initialize')!;
		expect(init.params?.clientCapabilities).toEqual({});
		const wire = ch.lines.join('\n');
		expect(wire).not.toContain('_meta');
		expect(wire).not.toContain('authenticate');
		expect(wire).not.toContain('"fs"');
		expect(wire).not.toContain('"terminal"');
		expect(ch.received.map((m) => m.method)).toEqual([
			'initialize',
			'session/new',
			'session/prompt',
			'session/cancel'
		]);
	});

	it('uses small integer request ids', async () => {
		const { ch, client } = setup();
		await client.initialize();
		await client.newSession();
		await client.prompt('s', 'x');
		const ids = ch.received.filter((m) => m.id !== undefined).map((m) => m.id);
		expect(ids.every((id) => Number.isSafeInteger(id) && id! >= 0 && id! < 1000)).toBe(true);
		expect(new Set(ids).size).toBe(ids.length);
	});

	it('cancel is a notification (no id)', async () => {
		const { ch, client } = setup();
		await client.cancel('s');
		await tick();
		expect(ch.received.at(-1)).toMatchObject({ method: 'session/cancel' });
		expect(ch.received.at(-1)!.id).toBeUndefined();
	});

	it('set_mode accepts only default and plan', async () => {
		const { ch, client } = setup();
		await client.setMode('s', 'plan');
		await client.setMode('s', 'default');
		await expect(client.setMode('s', 'bypassPermissions' as never)).rejects.toBeInstanceOf(
			AcpLimitError
		);
		expect(ch.received.filter((m) => m.method === 'session/set_mode')).toHaveLength(2);
	});

	it('rejects an over-cap prompt without writing it', async () => {
		const { ch, client } = setup();
		await client.initialize();
		const before = ch.lines.length;
		await expect(client.prompt('s', 'a'.repeat(MAX_PROMPT_BYTES + 1))).rejects.toBeInstanceOf(
			AcpLimitError
		);
		expect(ch.lines.length).toBe(before);
		// multi-byte text is measured in bytes
		await expect(client.prompt('s', '€'.repeat(MAX_PROMPT_BYTES / 3 + 1))).rejects.toBeInstanceOf(
			AcpLimitError
		);
		await expect(client.prompt('s', 'a'.repeat(MAX_PROMPT_BYTES))).resolves.toBe('end_turn');
	});

	it('refuses the 65th concurrent request', async () => {
		const { ch, client } = setup({ onMessage: (m) => m.method === 'session/prompt' });
		const open = Array.from({ length: MAX_PENDING }, () => client.prompt('s', 'x'));
		await expect(client.prompt('s', 'x')).rejects.toBeInstanceOf(AcpLimitError);
		await tick();
		expect(ch.received.filter((m) => m.method === 'session/prompt')).toHaveLength(MAX_PENDING);
		void open;
	});
});

describe('inbound surface', () => {
	it('routes session/update', async () => {
		const { ch, handlers } = setup();
		ch.send({
			jsonrpc: '2.0',
			method: 'session/update',
			params: {
				sessionId: 's',
				update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'x' } }
			}
		});
		await tick();
		expect(handlers.sessionUpdate).toHaveBeenCalledWith(
			's',
			expect.objectContaining({ sessionUpdate: 'agent_message_chunk' })
		);
	});

	it('trusts _enzarb/permission_denied only; other ext notifications are ignored', async () => {
		const { ch, handlers } = setup();
		ch.send({ jsonrpc: '2.0', method: '_enzarb/permission_denied', params: { a: 1 } });
		ch.send({ jsonrpc: '2.0', method: '_enzarb/evil', params: { a: 2 } });
		ch.send({ jsonrpc: '2.0', method: '_other/thing', params: {} });
		await tick();
		expect(handlers.permissionDenied).toHaveBeenCalledTimes(1);
		expect(handlers.permissionDenied).toHaveBeenCalledWith({ a: 1 });
		expect(handlers.sessionUpdate).not.toHaveBeenCalled();
	});

	it('answers permission requests with the handler outcome', async () => {
		const { ch } = setup(
			{},
			{ requestPermission: async () => ({ outcome: 'selected', optionId: 'a' }) }
		);
		ch.send({
			jsonrpc: '2.0',
			id: 7,
			method: 'session/request_permission',
			params: { sessionId: 's', toolCall: { toolCallId: 't' }, options: [] }
		});
		await tick();
		expect(ch.received.at(-1)).toMatchObject({
			id: 7,
			result: { outcome: { outcome: 'selected', optionId: 'a' } }
		});
	});

	it.each([
		['fs/read_text_file', { sessionId: 's', path: '/etc/passwd' }],
		['fs/write_text_file', { sessionId: 's', path: '/x', content: 'y' }],
		['terminal/create', { sessionId: 's', command: 'sh' }],
		['terminal/kill', { sessionId: 's', terminalId: 't' }]
	])('rejects %s with method-not-found', async (method, params) => {
		const { ch } = setup();
		ch.send({ jsonrpc: '2.0', id: 9, method, params });
		await tick();
		expect(ch.received.at(-1)).toMatchObject({ id: 9, error: { code: -32601 } });
	});

	it('rejects unknown ext requests with method-not-found', async () => {
		const { ch } = setup();
		ch.send({ jsonrpc: '2.0', id: 9, method: '_enzarb/anything', params: {} });
		await tick();
		expect(ch.received.at(-1)).toMatchObject({ id: 9, error: { code: -32601 } });
	});
});
