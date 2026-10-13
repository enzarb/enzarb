import { CloseReason } from '@enzarb/proto';
import { describe, expect, it, vi } from 'vitest';
import {
	BACKOFF_BASE_MS,
	BACKOFF_MAX_MS,
	MAX_RETRIES,
	createChatConnection,
	type AcpChannel
} from './connection.ts';
import { sandboxConnector, type MuxSession } from './sandbox-connect.ts';
import { fakeAgentChannel, type FakeChannel } from '../test/acp-fake.ts';

const flush = async () => {
	for (let i = 0; i < 8; i++) await new Promise((r) => setTimeout(r, 0));
};

/** Manual timers: nothing runs until `fire`, and `delays` shows what is scheduled. */
class Timers {
	private next = 1;
	private pending = new Map<number, { ms: number; fn: () => void }>();
	setTimeout = (fn: () => void, ms: number) => {
		const id = this.next++;
		this.pending.set(id, { ms, fn });
		return id;
	};
	clearTimeout = (h: unknown) => void this.pending.delete(h as number);
	get delays(): number[] {
		return [...this.pending.values()].map((t) => t.ms);
	}
	async fire() {
		const [id, t] = [...this.pending.entries()][0];
		this.pending.delete(id);
		t.fn();
		await flush();
	}
}

class FakeSocket extends EventTarget {
	closed = false;
	constructor(public url: string) {
		super();
	}
	close() {
		this.closed = true;
	}
}

type Sess = MuxSession & { close: ReturnType<typeof vi.fn> };

function setup(openAcpImpl: (n: number) => Promise<FakeChannel>) {
	const timers = new Timers();
	const channels: FakeChannel[] = [];
	const sessions: Sess[] = [];
	const sockets: FakeSocket[] = [];
	let opens = 0;
	const openAcp = vi.fn(async () => {
		const ch = await openAcpImpl(opens++);
		channels.push(ch);
		return ch as unknown as AcpChannel;
	});
	const connector = sandboxConnector('alpha', {
		connection: async (p) => ({ url: `wss://x/sandboxes/${p}/mux?t=tok` }),
		openSocket: (url) => {
			const ws = new FakeSocket(url);
			sockets.push(ws);
			queueMicrotask(() => ws.dispatchEvent(new Event('open')));
			return ws as unknown as WebSocket;
		},
		handshake: async () => {
			await Promise.resolve();
			const s: Sess = {
				features: new Set(['acp']),
				openAcp: openAcp as unknown as MuxSession['openAcp'],
				closed: new Promise<unknown>(() => {}),
				close: vi.fn(async () => {})
			};
			sessions.push(s);
			return s;
		}
	});
	const visible = { value: 'visible' as 'visible' | 'hidden', listeners: new Set<() => void>() };
	const conn = createChatConnection({
		connect: connector.connect,
		setTimeout: timers.setTimeout,
		clearTimeout: timers.clearTimeout,
		random: () => 0,
		visibility: {
			state: () => visible.value,
			onChange: (cb) => {
				visible.listeners.add(cb);
				return () => visible.listeners.delete(cb);
			}
		}
	});
	const show = () => {
		visible.value = 'visible';
		visible.listeners.forEach((l) => l());
	};
	return { conn, connector, timers, channels, sessions, sockets, openAcp, visible, show };
}

const agent = () => fakeAgentChannel({ sessionId: 's1' });

describe('reconnect through the real connector', () => {
	it('an unexpected close retries MAX_RETRIES times with doubling, capped backoff, then fails', async () => {
		const t = setup(async (n) => {
			if (n === 0) return agent();
			throw new Error('agent down');
		});
		t.conn.start();
		await flush();
		expect(t.conn.state().phase).toBe('ready');

		t.channels[0].end('exit:1');
		await flush();
		const seen: number[] = [];
		for (let i = 0; i < MAX_RETRIES; i++) {
			expect(t.conn.state().phase).toBe('reconnecting');
			expect(t.timers.delays).toHaveLength(1);
			seen.push(t.timers.delays[0]);
			await t.timers.fire();
		}
		expect(seen).toEqual(
			Array.from({ length: MAX_RETRIES }, (_, i) =>
				Math.min(BACKOFF_MAX_MS, BACKOFF_BASE_MS * 2 ** i)
			)
		);
		expect(seen.at(-1)).toBe(BACKOFF_MAX_MS);
		const st = t.conn.state();
		expect(st.phase).toBe('failed');
		expect(st.action).toBe('reconnect');
		expect(t.timers.delays).toEqual([]);
		// One initial open plus one per retry, all on the same mux session.
		expect(t.openAcp).toHaveBeenCalledTimes(1 + MAX_RETRIES);
		expect(t.sessions).toHaveLength(1);
	});

	it('a recovered retry resets the attempt counter', async () => {
		const t = setup(async () => agent());
		t.conn.start();
		await flush();
		t.channels[0].end('exit:1');
		await flush();
		expect(t.timers.delays).toEqual([BACKOFF_BASE_MS]);
		await t.timers.fire();
		expect(t.conn.state()).toMatchObject({ phase: 'ready', attempt: 0 });
		t.channels[1].end('exit:1');
		await flush();
		expect(t.timers.delays).toEqual([BACKOFF_BASE_MS]);
	});

	it('shutdown reconnects only while the page is visible', async () => {
		const t = setup(async () => agent());
		t.conn.start();
		await flush();
		t.visible.value = 'hidden';
		t.channels[0].end(CloseReason.Shutdown);
		await flush();
		expect(t.conn.state().phase).toBe('restarting');
		expect(t.timers.delays).toEqual([]);

		// A visibility event while still hidden changes nothing.
		t.visible.listeners.forEach((l) => l());
		expect(t.timers.delays).toEqual([]);

		t.show();
		expect(t.conn.state().phase).toBe('restarting');
		expect(t.timers.delays).toEqual([BACKOFF_BASE_MS]);
		await t.timers.fire();
		expect(t.conn.state().phase).toBe('ready');
		expect(t.openAcp).toHaveBeenCalledTimes(2);
	});

	it('shutdown while visible schedules the retry straight away', async () => {
		const t = setup(async () => agent());
		t.conn.start();
		await flush();
		t.channels[0].end(CloseReason.Shutdown);
		await flush();
		expect(t.conn.state().phase).toBe('restarting');
		expect(t.timers.delays).toEqual([BACKOFF_BASE_MS]);
	});

	it('limit is terminal: no timer, no reconnect, until the user asks', async () => {
		const t = setup(async () => agent());
		t.conn.start();
		await flush();
		t.channels[0].end(CloseReason.Limit);
		await flush();
		expect(t.conn.state()).toMatchObject({ phase: 'limit', action: 'reconnect' });
		expect(t.timers.delays).toEqual([]);
		t.show();
		await flush();
		expect(t.conn.state().phase).toBe('limit');
		expect(t.openAcp).toHaveBeenCalledTimes(1);

		t.conn.reconnect();
		await flush();
		expect(t.conn.state().phase).toBe('ready');
		expect(t.openAcp).toHaveBeenCalledTimes(2);
	});

	it('closes the previous channel on a manual reconnect and everything on dispose', async () => {
		const t = setup(async () => agent());
		t.conn.start();
		await flush();
		t.conn.reconnect();
		await flush();
		expect(t.channels[0].clientClosed()).toBe(true);
		expect(t.channels[1].clientClosed()).toBe(false);
		expect(t.conn.state().phase).toBe('ready');

		t.conn.dispose();
		t.connector.dispose();
		await flush();
		expect(t.channels[1].clientClosed()).toBe(true);
		expect(t.sessions[0].close).toHaveBeenCalled();
	});
});
