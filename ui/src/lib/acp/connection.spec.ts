import { describe, expect, it, vi } from 'vitest';
import { load as loadStoredId } from './session-store.ts';
import { CloseReason, OpenError } from '@enzarb/proto';
import {
	BACKOFF_MAX_MS,
	OPEN_TIMEOUT_MS,
	backoffDelay,
	createChatConnection,
	decideClose,
	describeOpenError,
	type AcpChannel,
	type AcpSession,
	type ChatConnection
} from './connection.ts';
import { buildChatSteps } from '../chat-steps.ts';
import { fakeAgentChannel, type AgentOpts, type FakeChannel } from '../test/acp-fake.ts';

class Clock {
	now = 0;
	private next = 1;
	private timers = new Map<number, { at: number; fn: () => void }>();
	setTimeout = (fn: () => void, ms: number) => {
		const id = this.next++;
		this.timers.set(id, { at: this.now + ms, fn });
		return id;
	};
	clearTimeout = (h: unknown) => void this.timers.delete(h as number);
	get pending(): number[] {
		return [...this.timers.values()].map((t) => t.at - this.now).sort((a, b) => a - b);
	}
	async advance(ms: number) {
		const end = this.now + ms;
		for (;;) {
			const due = [...this.timers.entries()]
				.filter(([, t]) => t.at <= end)
				.sort((a, b) => a[1].at - b[1].at)[0];
			if (!due) break;
			this.timers.delete(due[0]);
			this.now = due[1].at;
			due[1].fn();
			await flush();
		}
		this.now = end;
		await flush();
	}
}

const flush = async () => {
	for (let i = 0; i < 8; i++) await new Promise((r) => setTimeout(r, 0));
};

interface Harness {
	conn: ChatConnection;
	clock: Clock;
	channels: FakeChannel[];
	openAcp: ReturnType<typeof vi.fn>;
	visible: {
		value: 'visible' | 'hidden';
		listeners: Set<() => void>;
		set(v: 'visible' | 'hidden'): void;
	};
	session: AcpSession;
}

function harness(
	o: {
		features?: string[];
		agent?: AgentOpts | ((n: number) => AgentOpts);
		open?: (n: number, opts: unknown) => Promise<FakeChannel>;
		repo?: string;
		initialSessionId?: string;
		onSessionId?: (id: string | null) => void;
	} = {}
): Harness {
	const clock = new Clock();
	const channels: FakeChannel[] = [];
	const visible = {
		value: 'visible' as 'visible' | 'hidden',
		listeners: new Set<() => void>(),
		set(v: 'visible' | 'hidden') {
			visible.value = v;
			visible.listeners.forEach((l) => l());
		}
	};
	const openAcp = vi.fn(async (opts?: unknown) => {
		const n = channels.length;
		if (o.open) {
			const ch = await o.open(n, opts);
			channels.push(ch);
			return ch as unknown as AcpChannel;
		}
		const agent = typeof o.agent === 'function' ? o.agent(n) : (o.agent ?? {});
		const ch = fakeAgentChannel({ sessionId: `sess-${n + 1}`, ...agent });
		channels.push(ch);
		return ch as unknown as AcpChannel;
	});
	const session: AcpSession = {
		features: new Set(o.features ?? ['acp']),
		openAcp: openAcp as unknown as AcpSession['openAcp']
	};
	const conn = createChatConnection({
		connect: async (report) => {
			report('token');
			report('connect');
			return session;
		},
		repo: o.repo,
		initialSessionId: o.initialSessionId,
		onSessionId: o.onSessionId,
		setTimeout: clock.setTimeout,
		clearTimeout: clock.clearTimeout,
		random: () => 0,
		visibility: {
			state: () => visible.value,
			onChange: (cb) => {
				visible.listeners.add(cb);
				return () => visible.listeners.delete(cb);
			}
		}
	});
	return { conn, clock, channels, openAcp, visible, session };
}

async function ready(h: Harness) {
	h.conn.start();
	await flush();
	expect(h.conn.state().phase).toBe('ready');
}

describe('features check', () => {
	it('an old proxy (no features) never gets open acp', async () => {
		const h = harness({ features: [] });
		h.conn.start();
		await flush();
		expect(h.openAcp).not.toHaveBeenCalled();
		const st = h.conn.state();
		expect(st.phase).toBe('unavailable');
		expect(st.message).toBe('Chat is not available');
		expect(st.action).toBeNull();
		expect(buildChatSteps(st.progress)[3].state).toBe('error');
	});

	it('opens acp when the feature is advertised', async () => {
		const h = harness({ repo: 'o/r' });
		await ready(h);
		expect(h.openAcp).toHaveBeenCalledWith({ repo: 'o/r' });
		expect(buildChatSteps(h.conn.state().progress).every((s) => s.state === 'done')).toBe(true);
	});
});

describe('open errors', () => {
	it.each([
		['chat_disabled', 'Chat is disabled for this project', null],
		['chat_limit', 'A chat is already open on this connection', 'retry'],
		['no_credential', 'No Claude credential is connected for this project', 'retry'],
		['repo_not_cloned', 'The repository is not cloned in the sandbox yet', 'retry'],
		['unsupported_kind', 'This sandbox does not support chat', null],
		['invalid_request', 'The chat request was invalid', null],
		['something_new', 'Chat could not be started', 'retry'],
		[undefined, 'Chat could not be started', 'retry']
	])('%s fails "Start agent" and is terminal', async (code, message, action) => {
		const h = harness({
			open: async () => {
				throw new OpenError('open refused', code, '<img src=x onerror=alert(1)>');
			}
		});
		h.conn.start();
		await flush();
		await h.clock.advance(120_000);
		const st = h.conn.state();
		expect(st.phase).toBe('failed');
		expect(st.message).toBe(message);
		expect(st.action).toBe(action);
		expect(h.openAcp).toHaveBeenCalledTimes(1);
		expect(buildChatSteps(st.progress)[3]).toMatchObject({ state: 'error', message });
		expect(JSON.stringify(st)).not.toContain('onerror');
	});

	it('describeOpenError ignores inherited keys', () => {
		expect(describeOpenError('constructor').message).toBe('Chat could not be started');
	});
});

describe('slow reopen', () => {
	it('a 20 s open keeps "Start agent" in progress with no error', async () => {
		let release!: (c: FakeChannel) => void;
		const h = harness({ open: () => new Promise((r) => (release = r)) });
		h.conn.start();
		await flush();
		await h.clock.advance(20_000);
		let st = h.conn.state();
		expect(st.phase).toBe('connecting');
		expect(st.progress.failure).toBeNull();
		expect(buildChatSteps(st.progress)[3]).toMatchObject({
			state: 'active',
			label: 'Starting agent…'
		});
		release(fakeAgentChannel());
		await flush();
		st = h.conn.state();
		expect(st.phase).toBe('ready');
	});

	it('a 30 s hang fails with Retry and closes a late channel', async () => {
		let release!: (c: FakeChannel) => void;
		const h = harness({ open: () => new Promise((r) => (release = r)) });
		h.conn.start();
		await flush();
		await h.clock.advance(OPEN_TIMEOUT_MS);
		const st = h.conn.state();
		expect(st).toMatchObject({ phase: 'failed', message: 'Agent did not start', action: 'retry' });
		const late = fakeAgentChannel();
		release(late);
		await flush();
		expect(late.clientClosed()).toBe(true);
	});
});

describe('one live channel', () => {
	it('a reconnect waits for the old channel to close before openAcp', async () => {
		const h = harness();
		await ready(h);
		const first = h.channels[0];
		first.close = async () => {}; // the peer has not finished tearing down yet
		h.conn.reconnect();
		await flush();
		expect(h.openAcp).toHaveBeenCalledTimes(1);
		first.end('closed');
		await flush();
		expect(h.openAcp).toHaveBeenCalledTimes(2);
		expect(h.conn.state().phase).toBe('ready');
	});
});

describe('close reasons', () => {
	it('limit is terminal with a manual Reconnect and no auto retry', async () => {
		const h = harness();
		await ready(h);
		h.channels[0].end('limit');
		await flush();
		await h.clock.advance(300_000);
		expect(h.conn.state()).toMatchObject({ phase: 'limit', action: 'reconnect' });
		expect(h.openAcp).toHaveBeenCalledTimes(1);
		h.conn.reconnect();
		await flush();
		expect(h.openAcp).toHaveBeenCalledTimes(2);
	});

	it('superseded shows Take over, which reconnects', async () => {
		const h = harness();
		await ready(h);
		h.channels[0].end('superseded');
		await flush();
		await h.clock.advance(300_000);
		expect(h.conn.state()).toMatchObject({
			phase: 'superseded',
			message: 'Chat opened in another tab',
			action: 'take_over'
		});
		expect(h.openAcp).toHaveBeenCalledTimes(1);
		h.conn.reconnect();
		await flush();
		expect(h.conn.state().phase).toBe('ready');
	});

	it('idle waits for the next user action, then reconnects and sends it', async () => {
		const h = harness();
		await ready(h);
		h.channels[0].end('idle');
		await flush();
		await h.clock.advance(300_000);
		expect(h.conn.state().phase).toBe('disconnected');
		expect(h.openAcp).toHaveBeenCalledTimes(1);
		h.conn.send('hello again');
		await flush();
		expect(h.openAcp).toHaveBeenCalledTimes(2);
		expect(h.channels[1].received.some((m) => m.method === 'session/prompt')).toBe(true);
	});

	it('shutdown reconnects with backoff while visible', async () => {
		const h = harness();
		await ready(h);
		h.channels[0].end('shutdown');
		await flush();
		expect(h.conn.state()).toMatchObject({ phase: 'restarting', message: 'Sandbox restarting' });
		await h.clock.advance(1_000);
		expect(h.openAcp).toHaveBeenCalledTimes(2);
	});

	it('shutdown waits while hidden and reconnects when the page becomes visible', async () => {
		const h = harness();
		await ready(h);
		h.visible.value = 'hidden';
		h.channels[0].end('shutdown');
		await flush();
		await h.clock.advance(300_000);
		expect(h.conn.state().phase).toBe('restarting');
		expect(h.openAcp).toHaveBeenCalledTimes(1);
		h.visible.set('visible');
		await h.clock.advance(1_000);
		expect(h.openAcp).toHaveBeenCalledTimes(2);
	});

	it.each([['exit:1'], ['signal:9'], ['who knows'], ['']])(
		'unexpected close %j backs off 1 s',
		async (reason) => {
			const h = harness();
			await ready(h);
			h.channels[0].end(reason);
			await flush();
			expect(h.conn.state().phase).toBe('reconnecting');
			await h.clock.advance(999);
			expect(h.openAcp).toHaveBeenCalledTimes(1);
			await h.clock.advance(1);
			expect(h.openAcp).toHaveBeenCalledTimes(2);
			expect(h.conn.state().attempt).toBe(0); // reset by the successful session
		}
	);

	it('retries 1, 2, 4, 8, 16, 30 s and then gives up with a manual Reconnect', async () => {
		const h = harness({
			open: async (n) => {
				if (n === 0) return fakeAgentChannel();
				throw new Error('socket gone');
			}
		});
		await ready(h);
		h.channels[0].end('exit:1');
		await flush();
		const delays: number[] = [];
		for (let i = 0; i < 6; i++) {
			const [next] = h.clock.pending;
			delays.push(next);
			await h.clock.advance(next);
		}
		expect(delays).toEqual([1000, 2000, 4000, 8000, 16000, 30000]);
		expect(h.openAcp).toHaveBeenCalledTimes(7);
		await h.clock.advance(600_000);
		expect(h.openAcp).toHaveBeenCalledTimes(7);
		expect(h.conn.state()).toMatchObject({ phase: 'failed', action: 'reconnect' });
	});

	it('decideClose / backoffDelay tables', () => {
		expect(decideClose('idle').kind).toBe('wait');
		expect(decideClose('shutdown').kind).toBe('restart');
		expect(decideClose(undefined).kind).toBe('backoff');
		expect(backoffDelay(10, () => 0)).toBe(BACKOFF_MAX_MS);
		expect(backoffDelay(0, () => 1)).toBe(800);
	});
});

describe('resume', () => {
	const crash = async (h: Harness) => {
		h.channels[0].end('exit:1');
		await flush();
		await h.clock.advance(1_000);
	};

	it('a first connect has no resume and no notice', async () => {
		const h = harness();
		await ready(h);
		expect(h.openAcp).toHaveBeenCalledWith({});
		expect(h.conn.state().chat.items.some((i) => i.kind === 'notice')).toBe(false);
	});

	it('passes the ACP session id and loads that same session when advertised', async () => {
		const h = harness({ agent: { loadSession: true } });
		await ready(h);
		expect(h.conn.state().sessionId).toBe('sess-1');
		await crash(h);
		expect(h.openAcp).toHaveBeenLastCalledWith({ resume: 'sess-1' });
		const load = h.channels[1].received.find((m) => m.method === 'session/load');
		expect(load?.params?.sessionId).toBe('sess-1');
		expect(h.channels[1].received.some((m) => m.method === 'session/new')).toBe(false);
		expect(h.conn.state().sessionId).toBe('sess-1');
		expect(h.conn.state().chat.items.some((i) => i.kind === 'notice')).toBe(false);
	});

	it('replayed history rebuilds the transcript instead of duplicating it', async () => {
		const h = harness({
			agent: (n) => ({
				loadSession: true,
				sessionId: 'sess-1',
				load:
					n === 0
						? undefined
						: () => {
								h.channels[1].send({
									jsonrpc: '2.0',
									method: 'session/update',
									params: {
										sessionId: 'sess-1',
										update: {
											sessionUpdate: 'agent_message_chunk',
											content: { type: 'text', text: 'old' }
										}
									}
								});
								return {};
							}
			})
		});
		await ready(h);
		h.channels[0].send({
			jsonrpc: '2.0',
			method: 'session/update',
			params: {
				sessionId: 'sess-1',
				update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'old' } }
			}
		});
		await flush();
		await crash(h);
		await flush();
		expect(h.conn.state().chat.items.filter((i) => i.kind === 'agent')).toHaveLength(1);
	});

	it('shows not_restored and starts a new session when load is not advertised', async () => {
		const h = harness();
		await ready(h);
		h.channels[0].send({
			jsonrpc: '2.0',
			method: 'session/update',
			params: {
				sessionId: 'sess-1',
				update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'kept' } }
			}
		});
		await flush();
		await crash(h);
		expect(h.channels[1].received.some((m) => m.method === 'session/load')).toBe(false);
		const st = h.conn.state();
		expect(st.sessionId).toBe('sess-2');
		expect(st.chat.items.map((i) => i.kind)).toEqual(['agent', 'notice']);
	});

	it('a failing load falls back to a new session with the notice and keeps the old transcript', async () => {
		const h = harness({
			agent: (n) => ({ loadSession: true, load: n === 0 ? undefined : () => new Error('nope') })
		});
		await ready(h);
		h.channels[0].send({
			jsonrpc: '2.0',
			method: 'session/update',
			params: {
				sessionId: 'sess-1',
				update: { sessionUpdate: 'user_message_chunk', content: { type: 'text', text: 'kept' } }
			}
		});
		await flush();
		await crash(h);
		const st = h.conn.state();
		expect(h.channels[1].received.map((m) => m.method)).toContain('session/new');
		expect(st.phase).toBe('ready');
		expect(st.chat.items.map((i) => i.kind)).toEqual(['user', 'notice']);
		expect(st.sessionId).toBe('sess-2');
	});
});

describe('resume across reloads and New chat', () => {
	it.each(['a'.repeat(129), '', 'a b', 'a/b', 'a\u0001b', 'sess-é'])(
		'an invalid stored id (%j) gives a fresh open with {} and no resume',
		async (bad) => {
			const m = new Map([['enzarb.chat.p', bad]]);
			vi.stubGlobal('sessionStorage', {
				getItem: (k: string) => m.get(k) ?? null,
				removeItem: (k: string) => void m.delete(k)
			});
			try {
				const h = harness({ agent: { loadSession: true }, initialSessionId: loadStoredId('p') });
				await ready(h);
				expect(h.openAcp).toHaveBeenCalledWith({});
				expect(h.channels[0].received.some((x) => x.method === 'session/load')).toBe(false);
			} finally {
				vi.unstubAllGlobals();
			}
		}
	);

	it('offers the saved id as open.resume and loads it', async () => {
		const onSessionId = vi.fn();
		const h = harness({
			agent: { loadSession: true, sessionId: 'saved' },
			initialSessionId: 'saved',
			onSessionId
		});
		await ready(h);
		expect(h.openAcp).toHaveBeenCalledWith({ resume: 'saved' });
		expect(h.channels[0].received.some((m) => m.method === 'session/load')).toBe(true);
		expect(onSessionId).toHaveBeenCalledExactlyOnceWith('saved');
		expect(h.conn.state().chat.items.some((i) => i.kind === 'notice')).toBe(false);
	});

	it('an unknown id gives a fresh chat, the notice, and replaces the stored id', async () => {
		const onSessionId = vi.fn();
		const h = harness({
			agent: { loadSession: true, load: () => new Error('unknown session') },
			initialSessionId: 'stale',
			onSessionId
		});
		await ready(h);
		expect(h.conn.state().sessionId).toBe('sess-1');
		expect(h.conn.state().chat.items.map((i) => i.kind)).toEqual(['notice']);
		expect(onSessionId).toHaveBeenCalledExactlyOnceWith('sess-1');
	});

	it('reports a new session id on a first connect', async () => {
		const onSessionId = vi.fn();
		const h = harness({ onSessionId });
		await ready(h);
		expect(onSessionId).toHaveBeenCalledExactlyOnceWith('sess-1');
	});

	it('newChat clears the id first, then reopens without resume on a new channel', async () => {
		const calls: (string | null)[] = [];
		const h = harness({
			agent: { loadSession: true },
			onSessionId: (id) => calls.push(id)
		});
		await ready(h);
		h.conn.newChat();
		expect(calls).toEqual(['sess-1', null]);
		await flush();
		expect(h.openAcp).toHaveBeenLastCalledWith({});
		expect(h.channels[0].clientClosed()).toBe(true);
		const st = h.conn.state();
		expect(st.phase).toBe('ready');
		expect(st.sessionId).toBe('sess-2');
		expect(calls).toEqual(['sess-1', null, 'sess-2']);
		expect(st.chat.items).toEqual([]);
	});

	it('newChat drops a queued message', async () => {
		const h = harness();
		await ready(h);
		h.channels[0].end(CloseReason.Idle);
		await flush();
		expect(h.conn.state().phase).toBe('disconnected');
		h.conn.send('hello');
		h.conn.newChat();
		await flush();
		expect(h.conn.state().chat.items.some((i) => i.kind === 'user')).toBe(false);
	});
});

describe('permissions', () => {
	const request = (h: Harness, n: number, options: unknown, id = 50, meta?: unknown) =>
		h.channels[n].send({
			jsonrpc: '2.0',
			id,
			method: 'session/request_permission',
			params: {
				sessionId: h.conn.state().sessionId,
				toolCall: { toolCallId: 't1', title: 'Run rm' },
				options,
				_meta: meta
			}
		});
	const OPTS = [
		{ optionId: 'a', name: 'Allow', kind: 'allow_once' },
		{ optionId: 'aa', name: 'Always', kind: 'allow_always' },
		{ optionId: 'r', name: 'Reject', kind: 'reject_once' }
	];
	const perm = (h: Harness) =>
		h.conn.state().chat.items.find((i) => i.kind === 'permission') as
			(import('./types.ts').ChatItem & { kind: 'permission' }) | undefined;
	const response = (h: Harness, n: number, id = 50) =>
		h.channels[n].received.find((m) => m.id === id && 'result' in m)?.result as
			{ outcome: unknown } | undefined;

	it('allow once sends the same-request option id and never auto-approves', async () => {
		const h = harness();
		await ready(h);
		request(h, 0, OPTS, 50, { enzarb: { policy: 'deny', rule: 'no-rm', reason: 'x' } });
		await flush();
		const p = perm(h)!;
		expect(p.status).toBe('pending');
		expect(p.options.map((o) => o.optionId)).toEqual(['a', 'r']);
		expect(p.hint).toBe('Sandbox policy: would deny — no-rm');
		expect(response(h, 0)).toBeUndefined();
		h.conn.answer(p.id, 'allow', h.conn.state().epoch);
		await flush();
		expect(response(h, 0)?.outcome).toEqual({ outcome: 'selected', optionId: 'a' });
		expect(perm(h)!.status).toBe('allowed');
	});

	it('deny sends reject_once; a double click sends once', async () => {
		const h = harness();
		await ready(h);
		request(h, 0, OPTS);
		await flush();
		const { id } = perm(h)!;
		const epoch = h.conn.state().epoch;
		h.conn.answer(id, 'deny', epoch);
		h.conn.answer(id, 'deny', epoch);
		h.conn.answer(id, 'allow', epoch);
		await flush();
		expect(response(h, 0)?.outcome).toEqual({ outcome: 'selected', optionId: 'r' });
		expect(h.channels[0].received.filter((m) => m.id === 50 && 'result' in m)).toHaveLength(1);
		expect(perm(h)!.status).toBe('denied');
	});

	it('a close cancels pending items; a late click does nothing', async () => {
		const h = harness();
		await ready(h);
		request(h, 0, OPTS);
		await flush();
		const { id } = perm(h)!;
		const epoch = h.conn.state().epoch;
		h.channels[0].end('exit:1');
		await flush();
		expect(perm(h)!.status).toBe('cancelled');
		h.conn.answer(id, 'allow', epoch);
		expect(perm(h)!.status).toBe('cancelled');
	});

	it('a click carrying a stale epoch after reconnect does nothing', async () => {
		const h = harness();
		await ready(h);
		request(h, 0, OPTS);
		await flush();
		const { id } = perm(h)!;
		const oldEpoch = h.conn.state().epoch;
		h.conn.reconnect();
		await flush();
		expect(perm(h)!.status).toBe('cancelled');
		request(h, 1, OPTS, 51);
		await flush();
		const second = h.conn.state().chat.items.filter((i) => i.kind === 'permission')[1];
		h.conn.answer(second.id, 'allow', oldEpoch);
		expect(response(h, 1, 51)).toBeUndefined();
		expect(id).not.toBe(second.id);
	});

	it('Stop cancels the turn and every pending item', async () => {
		const h = harness({ agent: { onMessage: (m) => m.method === 'session/prompt' } });
		await ready(h);
		h.conn.send('go');
		await flush();
		expect(h.conn.state().busy).toBe(true);
		request(h, 0, OPTS);
		await flush();
		const { id } = perm(h)!;
		const epoch = h.conn.state().epoch;
		h.conn.stop();
		await flush();
		expect(
			h.channels[0].received.some((m) => m.method === 'session/cancel' && m.id === undefined)
		).toBe(true);
		expect(perm(h)!.status).toBe('cancelled');
		expect(response(h, 0)?.outcome).toEqual({ outcome: 'cancelled' });
		h.conn.answer(id, 'allow', epoch);
		expect(perm(h)!.status).toBe('cancelled');
	});

	it('dispose cancels pending items and closes the channel', async () => {
		const h = harness();
		await ready(h);
		request(h, 0, OPTS);
		await flush();
		h.conn.dispose();
		await flush();
		expect(perm(h)!.status).toBe('cancelled');
		expect(h.channels[0].clientClosed()).toBe(true);
	});

	it('an allow_always-only request has no usable options', async () => {
		const h = harness();
		await ready(h);
		request(h, 0, [OPTS[1]]);
		await flush();
		const p = perm(h)!;
		expect(p.options).toEqual([]);
		h.conn.answer(p.id, 'allow', h.conn.state().epoch);
		expect(perm(h)!.status).toBe('pending');
	});

	it('duplicate option ids are answered cancelled and rendered as cancelled', async () => {
		const h = harness();
		await ready(h);
		request(h, 0, [OPTS[0], { ...OPTS[2], optionId: 'a' }]);
		await flush();
		expect(perm(h)).toMatchObject({ status: 'cancelled', options: [] });
		expect(response(h, 0)?.outcome).toEqual({ outcome: 'cancelled' });
	});

	it('permission_denied becomes a blocked item only for the current session', async () => {
		const h = harness();
		await ready(h);
		const note = (sessionId: string) =>
			h.channels[0].send({
				jsonrpc: '2.0',
				method: '_enzarb/permission_denied',
				params: {
					sessionId,
					toolCall: { toolCallId: 'x', title: 'curl' },
					rule: 'net',
					reason: 'no network'
				}
			});
		note('other');
		await flush();
		expect(perm(h)).toBeUndefined();
		note('sess-1');
		await flush();
		expect(perm(h)).toMatchObject({ status: 'blocked', reason: 'no network', options: [] });
	});

	it('ignores session updates for another session', async () => {
		const h = harness();
		await ready(h);
		h.channels[0].send({
			jsonrpc: '2.0',
			method: 'session/update',
			params: {
				sessionId: 'other',
				update: { sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'x' } }
			}
		});
		await flush();
		expect(h.conn.state().chat.items).toEqual([]);
	});
});

describe('permission mode', () => {
	const modes = (current: string, ids = ['bypassPermissions', 'default', 'plan']) => ({
		currentModeId: current,
		availableModes: ids.map((id) => ({ id, name: id }))
	});
	const modeMsg = (sessionId: string, modeId: string, bypassAllowed = true) => ({
		jsonrpc: '2.0',
		method: '_enzarb/mode',
		params: { sessionId, modeId, bypassAllowed }
	});
	const modeUpdate = (sessionId: string, currentModeId: string) => ({
		jsonrpc: '2.0',
		method: 'session/update',
		params: { sessionId, update: { sessionUpdate: 'current_mode_update', currentModeId } }
	});

	it('is unknown without any mode data (old sandbox)', async () => {
		const h = harness();
		await ready(h);
		expect(h.conn.state().mode).toBeNull();
	});

	it('is seeded from the session/new response with bypassAllowed false', async () => {
		const h = harness({ agent: { modes: modes('bypassPermissions') } });
		await ready(h);
		expect(h.conn.state().mode).toEqual({
			modeId: 'bypassPermissions',
			bypassAllowed: false,
			available: ['bypassPermissions', 'default', 'plan']
		});
	});

	it('ignores an invalid response mode', async () => {
		const h = harness({ agent: { modes: modes('evil') } });
		await ready(h);
		expect(h.conn.state().mode).toBeNull();
	});

	it('is seeded from the session/load response', async () => {
		const h = harness({
			agent: { loadSession: true, modes: modes('plan', ['default', 'plan']) },
			initialSessionId: 'saved'
		});
		await ready(h);
		expect(h.conn.state().mode).toMatchObject({ modeId: 'plan', available: ['default', 'plan'] });
	});

	it('holds a mode event that precedes the session/new response and applies it on a match', async () => {
		const h = harness({
			agent: {
				sessionId: 'sess-1',
				modes: modes('bypassPermissions'),
				onMessage: (m, ch) => {
					if (m.method === 'session/new') ch.send(modeMsg('sess-1', 'default'));
				}
			}
		});
		await ready(h);
		expect(h.conn.state().mode).toEqual({
			modeId: 'default',
			bypassAllowed: true,
			available: ['bypassPermissions', 'default', 'plan']
		});
	});

	it('discards a held mode event for another session', async () => {
		const h = harness({
			agent: {
				sessionId: 'sess-1',
				modes: modes('plan'),
				onMessage: (m, ch) => {
					if (m.method === 'session/new') ch.send(modeMsg('other', 'default'));
				}
			}
		});
		await ready(h);
		expect(h.conn.state().mode).toMatchObject({ modeId: 'plan', bypassAllowed: false });
	});

	it('applies a mode event after the response directly, and rejects other sessions', async () => {
		const h = harness({ agent: { sessionId: 'sess-1', modes: modes('bypassPermissions') } });
		await ready(h);
		h.channels[0].send(modeMsg('other', 'plan'));
		h.channels[0].send(modeMsg('sess-1', 'evil'));
		await flush();
		expect(h.conn.state().mode?.modeId).toBe('bypassPermissions');
		h.channels[0].send(modeMsg('sess-1', 'plan', true));
		await flush();
		expect(h.conn.state().mode).toMatchObject({ modeId: 'plan', bypassAllowed: true });
		h.channels[0].send(modeUpdate('sess-1', 'default'));
		await flush();
		expect(h.conn.state().mode).toMatchObject({ modeId: 'default', bypassAllowed: true });
	});

	it('stores current_mode_update with bypassAllowed false when nothing was seen', async () => {
		const h = harness({ agent: { sessionId: 'sess-1' } });
		await ready(h);
		h.channels[0].send(modeUpdate('sess-1', 'plan'));
		await flush();
		expect(h.conn.state().mode).toEqual({ modeId: 'plan', bypassAllowed: false, available: null });
	});

	it('applies a current_mode_update replayed during load, and the response does not overwrite it', async () => {
		const h = harness({
			agent: {
				loadSession: true,
				modes: modes('default', ['default', 'plan']),
				onMessage: (m, ch) => {
					if (m.method === 'session/load') {
						ch.send(modeUpdate('saved', 'plan'));
						ch.send(modeMsg('saved', 'plan', true));
					}
				}
			},
			initialSessionId: 'saved'
		});
		await ready(h);
		expect(h.conn.state().mode).toEqual({
			modeId: 'plan',
			bypassAllowed: true,
			available: ['default', 'plan']
		});
	});

	it('resets on New chat, reconnect and load failure', async () => {
		const h = harness({ agent: { modes: modes('plan') } });
		await ready(h);
		expect(h.conn.state().mode).not.toBeNull();
		h.conn.newChat();
		expect(h.conn.state().mode).toBeNull();
		await flush();
		expect(h.conn.state().mode?.modeId).toBe('plan');
		h.conn.reconnect();
		expect(h.conn.state().mode).toBeNull();
		await flush();

		const f = harness({
			agent: { loadSession: true, load: () => new Error('gone') },
			initialSessionId: 'saved'
		});
		await ready(f);
		expect(f.conn.state().mode).toBeNull();
	});

	describe('setMode', () => {
		const setModes = (h: Harness) =>
			h.channels[0].received.filter((m) => m.method === 'session/set_mode');

		it('changes the mode only after the sandbox confirms', async () => {
			let confirm: (() => void) | undefined;
			const h = harness({
				agent: {
					sessionId: 'sess-1',
					modes: modes('default'),
					onMessage: (m, ch) => {
						if (m.method !== 'session/set_mode') return;
						confirm = () => {
							ch.send({ jsonrpc: '2.0', id: m.id, result: {} });
							ch.send(modeMsg('sess-1', 'plan'));
						};
						return true;
					}
				}
			});
			await ready(h);
			h.conn.setMode('plan');
			await flush();
			expect(h.conn.state()).toMatchObject({ switching: true });
			expect(h.conn.state().mode?.modeId).toBe('default');
			h.conn.setMode('default');
			expect(setModes(h)).toHaveLength(1);
			expect(setModes(h)[0].params).toEqual({ sessionId: 'sess-1', modeId: 'plan' });
			confirm!();
			await flush();
			expect(h.conn.state()).toMatchObject({ switching: false });
			expect(h.conn.state().mode?.modeId).toBe('plan');
		});

		it('keeps the old mode and clears switching when rejected', async () => {
			const h = harness({
				agent: {
					modes: modes('default'),
					onMessage: (m, ch) => {
						if (m.method !== 'session/set_mode') return;
						ch.send({ jsonrpc: '2.0', id: m.id, error: { code: -32603, message: 'no' } });
						return true;
					}
				}
			});
			await ready(h);
			h.conn.setMode('plan');
			await flush();
			const st = h.conn.state();
			expect(st.mode?.modeId).toBe('default');
			expect(st.switching).toBe(false);
			expect(st.error).toBe('The mode could not be changed');
		});

		it('clears switching when set_mode resolves without a confirmation', async () => {
			const h = harness({ agent: { modes: modes('default') } });
			await ready(h);
			h.conn.setMode('plan');
			await flush();
			expect(h.conn.state().switching).toBe(false);
			expect(h.conn.state().mode?.modeId).toBe('default');
		});

		it('refuses a mode that was not advertised', async () => {
			const h = harness({ agent: { modes: modes('default', ['default']) } });
			await ready(h);
			h.channels[0].send(modeMsg('sess-1', 'default', true));
			await flush();
			h.conn.setMode('plan');
			await flush();
			expect(setModes(h)).toHaveLength(0);
		});

		it('refuses bypass while bypassAllowed is false, and allows it once true', async () => {
			const h = harness({ agent: { sessionId: 'sess-1', modes: modes('default') } });
			await ready(h);
			h.conn.setMode('bypassPermissions');
			await flush();
			expect(setModes(h)).toHaveLength(0);
			h.channels[0].send(modeMsg('sess-1', 'default', true));
			await flush();
			h.conn.setMode('bypassPermissions');
			await flush();
			expect(setModes(h)).toHaveLength(1);
		});

		it('does nothing while the mode is unknown', async () => {
			const h = harness();
			await ready(h);
			h.conn.setMode('plan');
			await flush();
			expect(setModes(h)).toHaveLength(0);
		});
	});
});
