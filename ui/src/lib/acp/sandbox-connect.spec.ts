import { describe, expect, it, vi } from 'vitest';
import { createChatConnection, type AcpChannel } from './connection.ts';
import { sandboxConnector, type MuxSession } from './sandbox-connect.ts';
import { fakeAgentChannel } from '../test/acp-fake.ts';

const flush = async () => {
	for (let i = 0; i < 8; i++) await new Promise((r) => setTimeout(r, 0));
};

class FakeSocket extends EventTarget {
	closed = false;
	constructor(public url: string) {
		super();
	}
	close() {
		this.closed = true;
	}
}

function setup(features: string[]) {
	const sockets: FakeSocket[] = [];
	const openAcp = vi.fn(async () => fakeAgentChannel({ sessionId: 's1' }) as unknown as AcpChannel);
	const sessions: (MuxSession & { close: ReturnType<typeof vi.fn> })[] = [];
	const connection = vi.fn(async (p: string) => ({ url: `wss://x/sandboxes/${p}/mux?t=tok` }));
	const connector = sandboxConnector('alpha', {
		connection,
		openSocket: (url) => {
			const ws = new FakeSocket(url);
			sockets.push(ws);
			queueMicrotask(() => ws.dispatchEvent(new Event('open')));
			return ws as unknown as WebSocket;
		},
		handshake: async () => {
			await Promise.resolve();
			const s = {
				features: new Set(features),
				openAcp,
				closed: new Promise<unknown>(() => {}),
				close: vi.fn(async () => {})
			};
			sessions.push(s);
			return s;
		}
	});
	const conn = createChatConnection({
		connect: connector.connect,
		setTimeout: (fn, ms) => setTimeout(fn, ms),
		clearTimeout: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
		random: () => 0,
		visibility: { state: () => 'visible', onChange: () => () => {} }
	});
	return { conn, connector, connection, sockets, sessions, openAcp };
}

describe('sandbox connector', () => {
	it('uses the terminal token for this project and never opens acp without the feature', async () => {
		const t = setup([]);
		t.conn.start();
		await flush();
		expect(t.connection).toHaveBeenCalledWith('alpha');
		expect(t.sockets[0].url).toBe('wss://x/sandboxes/alpha/mux?t=tok');
		expect(t.openAcp).not.toHaveBeenCalled();
		expect(t.conn.state().phase).toBe('unavailable');
		t.conn.dispose();
		t.connector.dispose();
		expect(t.sessions[0].close).toHaveBeenCalled();
	});

	it('opens acp when the sandbox advertises it', async () => {
		const t = setup(['acp']);
		t.conn.start();
		await flush();
		expect(t.openAcp).toHaveBeenCalledTimes(1);
		expect(t.conn.state().phase).toBe('ready');
		t.conn.dispose();
		t.connector.dispose();
	});

	it('hangs up the previous session when a new one connects', async () => {
		const t = setup([]);
		await t.connector.connect(() => {});
		await t.connector.connect(() => {});
		expect(t.sessions[0].close).toHaveBeenCalled();
		expect(t.sessions[1].close).not.toHaveBeenCalled();
		t.connector.dispose();
	});

	it('closes the socket when the handshake fails', async () => {
		const sockets: FakeSocket[] = [];
		const c = sandboxConnector('alpha', {
			connection: async () => ({ url: 'wss://x' }),
			openSocket: (url) => {
				const ws = new FakeSocket(url);
				sockets.push(ws);
				return ws as unknown as WebSocket;
			},
			handshake: async () => {
				throw new Error('bad welcome');
			}
		});
		await expect(c.connect(() => {})).rejects.toThrow('bad welcome');
		expect(sockets[0].closed).toBe(true);
	});

	it('closes a session that finishes the handshake after dispose', async () => {
		let release!: () => void;
		const close = vi.fn(async () => {});
		const c = sandboxConnector('alpha', {
			connection: async () => ({ url: 'wss://x' }),
			openSocket: (url) => new FakeSocket(url) as unknown as WebSocket,
			handshake: () =>
				new Promise((r) => {
					release = () =>
						r({ features: new Set(['acp']), openAcp: vi.fn(), close } as unknown as MuxSession);
				})
		});
		const p = c.connect(() => {});
		await flush();
		c.dispose();
		release();
		await expect(p).rejects.toThrow();
		expect(close).toHaveBeenCalled();
	});
});
