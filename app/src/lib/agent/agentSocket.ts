import { getAgentAuthToken } from '$lib/agentToken';
import type { AcpWsClientMsg, AcpWsEvent } from './types';

export type ConnState = 'connecting' | 'connected' | 'reconnecting' | 'failed';

/**
 * One WebSocket per ACP session, following the same JWT-in-Sec-WebSocket-Protocol
 * auth pattern as the terminal tab. All frames are JSON text (unlike terminal's
 * binary PTY stream), since ACP events are inherently structured.
 */
export class AgentSocket {
	private sock: WebSocket | undefined;
	private reconnectTimer: ReturnType<typeof setTimeout> | undefined;
	private reconnectAttempts = 0;
	private readonly maxReconnectAttempts = 6;
	private closed = false;

	state: ConnState = 'connecting';
	error = '';

	constructor(
		private agentBase: string,
		private sessionId: string,
		private onEvent: (event: AcpWsEvent) => void,
		private onStateChange: (state: ConnState, error: string) => void
	) {}

	private setState(state: ConnState, error = '') {
		this.state = state;
		this.error = error;
		this.onStateChange(state, error);
	}

	async connect() {
		if (this.closed) return;
		const token = await getAgentAuthToken();
		if (!token) {
			// Token minting can fail transiently (network blip, app redeploy);
			// retry with backoff instead of demanding a page reload.
			this.setState('reconnecting', 'Could not refresh credentials — retrying…');
			this.scheduleReconnect();
			return;
		}

		const wsUrl = `${this.agentBase.replace('https://', 'wss://').replace('http://', 'ws://')}/agent/sessions/${this.sessionId}/ws`;
		const sock = new WebSocket(wsUrl, ['bearer', token]);
		this.sock = sock;

		sock.onopen = () => {
			this.reconnectAttempts = 0;
			this.setState('connected');
		};
		sock.onerror = () => {
			this.setState(this.state, 'WebSocket error — check that the workspace is running and reachable.');
		};
		sock.onclose = (e) => {
			if (this.closed) return;
			this.sock = undefined;
			if (e.code === 1000 || e.code === 1001) return;
			if (e.code === 4404) {
				this.setState('failed', 'Session not found — it may have been deleted or the workspace restarted.');
				return;
			}
			this.setState('reconnecting', `Disconnected (code ${e.code}) — attempting to reconnect…`);
			this.scheduleReconnect();
		};
		sock.onmessage = (e) => {
			try {
				const event = JSON.parse(e.data) as AcpWsEvent;
				this.onEvent(event);
			} catch {
				/* ignore malformed frames */
			}
		};
	}

	private scheduleReconnect() {
		clearTimeout(this.reconnectTimer);
		this.reconnectAttempts += 1;
		if (this.reconnectAttempts > this.maxReconnectAttempts) {
			this.setState('failed', 'Reconnection failed — the workspace may be unavailable.');
			return;
		}
		const delay = Math.min(1500 * 2 ** (this.reconnectAttempts - 1), 30_000);
		this.reconnectTimer = setTimeout(() => {
			if (!this.closed) this.connect();
		}, delay);
	}

	send(msg: AcpWsClientMsg) {
		if (this.sock?.readyState === WebSocket.OPEN) {
			this.sock.send(JSON.stringify(msg));
		}
	}

	close() {
		this.closed = true;
		clearTimeout(this.reconnectTimer);
		if (this.sock) {
			this.sock.onclose = null;
			this.sock.onmessage = null;
			this.sock.onerror = null;
			this.sock.close();
			this.sock = undefined;
		}
	}
}
