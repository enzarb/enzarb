import type { AcpSession, ConnectReport } from './connection.ts';

/** A connected mux session as the chat needs it: the ACP slice plus a way to hang up. */
export type MuxSession = AcpSession & { close(): Promise<unknown> };

export interface SandboxConnectDeps {
	/** Mints the short-lived proxy token and URL (the terminal's `terminalConnection`). */
	connection(project: string): Promise<{ url: string }>;
	/** Opens the websocket; the browser's `new WebSocket(url)`. */
	openSocket(url: string): WebSocket;
	/** proto's `Session.connect(ws, { client: 'ui' })`. */
	handshake(ws: WebSocket): Promise<MuxSession>;
}

/**
 * The chat's path to the sandbox, the same as the terminal's: token, websocket, handshake. Every
 * session it opens is closed by `dispose` (the connection itself only closes its ACP channel).
 * Whether `open acp` may be sent is the connection's decision (`features.has('acp')`).
 */
export function sandboxConnector(project: string, deps: SandboxConnectDeps) {
	const sessions = new Set<MuxSession>();
	let disposed = false;

	async function connect(report: ConnectReport): Promise<AcpSession> {
		const { url } = await deps.connection(project);
		report('token');
		const ws = deps.openSocket(url);
		ws.addEventListener('open', () => report('connect'));
		let session: MuxSession;
		try {
			session = await deps.handshake(ws);
		} catch (e) {
			ws.close();
			throw e;
		}
		if (disposed) {
			void session.close().catch(() => {});
			throw new Error('disposed');
		}
		// The connection uses one session at a time: an earlier one (stale or replaced) is hung up.
		for (const old of sessions) void old.close().catch(() => {});
		sessions.clear();
		sessions.add(session);
		void session.closed?.then(() => sessions.delete(session));
		report('handshake');
		return session;
	}

	function dispose(): void {
		disposed = true;
		for (const s of sessions) void s.close().catch(() => {});
		sessions.clear();
	}

	return { connect, dispose };
}
