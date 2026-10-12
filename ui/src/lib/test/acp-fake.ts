// Test-only: an in-memory ACP agent behind the same shape as proto's `ByteChannel`.
export type Msg = {
	id?: number;
	method?: string;
	params?: Record<string, unknown>;
	[k: string]: unknown;
};

export interface FakeChannel {
	id: number;
	readable: ReadableStream<Uint8Array>;
	writable: WritableStream<Uint8Array>;
	closed: Promise<{ code: number | null; reason: string }>;
	close: () => Promise<void>;
	/** Every line the client wrote, verbatim. */
	lines: string[];
	/** Parsed messages the client sent. */
	received: Msg[];
	/** Push an agent -> client message. */
	send: (m: unknown) => void;
	/** The peer closes the channel with `reason`. */
	end: (reason?: string) => void;
	/** Client closed its writable (half-close). */
	clientClosed: () => boolean;
}

export interface AgentOpts {
	loadSession?: boolean;
	/** Result for `session/load` (an Error rejects it). */
	load?: (params: Record<string, unknown>) => unknown | Error;
	sessionId?: string;
	/** Called for each request/notification before the default reply; return true to suppress it. */
	onMessage?: (m: Msg, ch: FakeChannel) => boolean | void;
}

let nextId = 1;

export function fakeAgentChannel(opts: AgentOpts = {}): FakeChannel {
	const enc = new TextEncoder();
	const dec = new TextDecoder();
	let ctl!: ReadableStreamDefaultController<Uint8Array>;
	let resolveClosed!: (v: { code: number | null; reason: string }) => void;
	let clientClosed = false;
	let buf = '';
	const lines: string[] = [];
	const received: Msg[] = [];
	const ch: FakeChannel = {
		id: nextId++,
		lines,
		received,
		readable: new ReadableStream<Uint8Array>({ start: (c) => void (ctl = c) }),
		closed: new Promise((r) => void (resolveClosed = r)),
		writable: undefined as unknown as WritableStream<Uint8Array>,
		close: async () => {
			clientClosed = true;
			ch.end('closed');
		},
		send: (m) => ctl.enqueue(enc.encode(JSON.stringify(m) + '\n')),
		end: (reason = '') => {
			try {
				ctl.close();
			} catch {
				/* already closed */
			}
			resolveClosed({ code: null, reason });
		},
		clientClosed: () => clientClosed
	};
	const reply = (m: Msg, result: unknown) => ch.send({ jsonrpc: '2.0', id: m.id, result });
	const onLine = (line: string) => {
		lines.push(line);
		const m = JSON.parse(line) as Msg;
		received.push(m);
		if (opts.onMessage?.(m, ch) === true || m.id === undefined || m.method === undefined) return;
		switch (m.method) {
			case 'initialize':
				reply(m, {
					protocolVersion: 1,
					agentCapabilities: { loadSession: opts.loadSession ?? false }
				});
				break;
			case 'session/new':
				reply(m, { sessionId: opts.sessionId ?? 'sess-new' });
				break;
			case 'session/load': {
				const r = opts.load?.(m.params ?? {});
				if (r instanceof Error) {
					ch.send({ jsonrpc: '2.0', id: m.id, error: { code: -32603, message: r.message } });
				} else reply(m, r ?? {});
				break;
			}
			case 'session/prompt':
				reply(m, { stopReason: 'end_turn' });
				break;
			default:
				reply(m, {});
		}
	};
	ch.writable = new WritableStream<Uint8Array>({
		write(chunk) {
			buf += dec.decode(chunk, { stream: true });
			let i;
			while ((i = buf.indexOf('\n')) >= 0) {
				const line = buf.slice(0, i);
				buf = buf.slice(i + 1);
				if (line) onLine(line);
			}
		},
		close() {
			clientClosed = true;
		}
	});
	return ch;
}
