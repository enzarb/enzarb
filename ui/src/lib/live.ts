/** Close code enzarb-realtime uses for "token rejected or expired: refetch a token and reconnect". */
export const CLOSE_REFETCH_TOKEN = 4001;
export const BACKOFF_BASE_MS = 1_000;
export const BACKOFF_MAX_MS = 30_000;
/** While the stream is down, pages are told to refresh this often. */
export const POLL_MS = 60_000;

/** The slice of the browser `WebSocket` the watcher uses. */
export interface LiveSocket {
	onopen: ((ev: unknown) => void) | null;
	onmessage: ((ev: { data: unknown }) => void) | null;
	onclose: ((ev: { code: number }) => void) | null;
	onerror: ((ev: unknown) => void) | null;
	close(): void;
}

export interface LiveDeps {
	WebSocket: new (url: string) => LiveSocket;
	/** Renews the HttpOnly realtime cookie server-side; returns the stream URL (never a token). */
	refresh: () => Promise<{ url: string; expiresInSeconds: number }>;
	setTimeout: (fn: () => void, ms: number) => unknown;
	clearTimeout: (handle: unknown) => void;
	random: () => number;
}

/** The cookie is renewed at this fraction of its lifetime. */
export const REFRESH_FRACTION = 0.8;

/**
 * Keeps a websocket to enzarb-realtime open. `onChange(id)` fires per logged event;
 * `onChange(null)` means "refresh everything": a `resync` frame, any reconnect (changes may have
 * been missed) and, while disconnected, a poll every minute. Each (re)connect renews the auth
 * cookie first, and it is renewed again before it expires.
 * Returns the unsubscribe function.
 */
export function createLiveWatcher(
	deps: LiveDeps,
	onChange: (id: number | null) => void
): () => void {
	let stopped = false;
	let socket: LiveSocket | null = null;
	let retryTimer: unknown = null;
	let pollTimer: unknown = null;
	let refreshTimer: unknown = null;
	let attempt = 0;
	let needsFull = false;
	let immediateUsed = false;

	function stopPoll() {
		if (pollTimer !== null) deps.clearTimeout(pollTimer);
		pollTimer = null;
	}

	function startPoll() {
		if (pollTimer !== null) return;
		const tick = () => {
			if (stopped) return;
			onChange(null);
			pollTimer = deps.setTimeout(tick, POLL_MS);
		};
		pollTimer = deps.setTimeout(tick, POLL_MS);
	}

	/** Renews the cookie and schedules the next renewal; there is only ever one pending timer. */
	async function refresh() {
		const res = await deps.refresh();
		if (stopped) return res;
		if (refreshTimer !== null) deps.clearTimeout(refreshTimer);
		refreshTimer = deps.setTimeout(
			() => {
				refreshTimer = null;
				if (stopped) return;
				// a failed timer refresh is not fatal: the next connect refreshes again
				refresh().catch(() => {});
			},
			Math.round(res.expiresInSeconds * 1000 * REFRESH_FRACTION)
		);
		return res;
	}

	function drop(code: number | undefined) {
		if (stopped) return;
		needsFull = true;
		startPoll();
		if (code === CLOSE_REFETCH_TOKEN && !immediateUsed) {
			immediateUsed = true;
			void connect();
			return;
		}
		const ceiling = Math.min(BACKOFF_MAX_MS, BACKOFF_BASE_MS * 2 ** attempt);
		attempt++;
		const delay = Math.round(ceiling * (0.5 + deps.random() / 2));
		retryTimer = deps.setTimeout(() => {
			retryTimer = null;
			void connect();
		}, delay);
	}

	async function connect() {
		let url: string;
		try {
			url = (await refresh()).url;
		} catch {
			drop(undefined);
			return;
		}
		if (stopped) return;
		const ws = new deps.WebSocket(url);
		socket = ws;
		let settled = false;
		const fail = (code?: number) => {
			if (settled) return;
			settled = true;
			if (socket === ws) socket = null;
			drop(code);
		};
		ws.onopen = () => {
			if (stopped) return;
			stopPoll();
			attempt = 0;
			if (needsFull) onChange(null);
			needsFull = false;
		};
		ws.onmessage = (e) => {
			if (stopped || typeof e.data !== 'string') return;
			immediateUsed = false;
			let frame: { topic?: unknown; id?: unknown };
			try {
				frame = JSON.parse(e.data);
			} catch {
				return;
			}
			if (frame === null || typeof frame !== 'object') return;
			if (frame.topic === 'event' && typeof frame.id === 'number') onChange(frame.id);
			else if (frame.topic === 'resync') onChange(null);
		};
		ws.onclose = (e) => fail(e.code);
		ws.onerror = () => {
			ws.close();
			fail();
		};
	}

	void connect();

	return () => {
		stopped = true;
		stopPoll();
		if (retryTimer !== null) deps.clearTimeout(retryTimer);
		retryTimer = null;
		if (refreshTimer !== null) deps.clearTimeout(refreshTimer);
		refreshTimer = null;
		socket?.close();
		socket = null;
	};
}

/** Subscribe to logged events over the realtime websocket. */
export function watchEvents(onChange: (id: number | null) => void): () => void {
	if (typeof WebSocket === 'undefined') return () => {};
	return createLiveWatcher(
		{
			WebSocket: WebSocket as unknown as LiveDeps['WebSocket'],
			// imported lazily so the watcher stays testable without the SvelteKit runtime
			refresh: async () => await (await import('#lib/events.remote.ts')).realtimeConnection(),
			setTimeout: (fn, ms) => setTimeout(fn, ms),
			clearTimeout: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
			random: Math.random
		},
		onChange
	);
}
