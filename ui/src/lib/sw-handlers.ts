// Pure logic of the service worker (`src/service-worker.ts`), kept free of worker globals so it can
// be unit tested. Nothing here may cache or touch authenticated responses.

export interface NotificationData {
	title: string;
	body: string;
	url: string;
	tag: string;
}

/** Paths that must always go to the network: authenticated remote calls and the realtime stream. */
const NEVER_CACHED = ['/_app/remote', '/live/', '/sandbox/', '/hooks/', '/login', '/logout'];

export const OFFLINE_PATH = '/offline';

/**
 * `url` as an in-app path (`/work/3?x=1`) when it is same-origin, otherwise null. Absolute URLs of
 * our own origin are reduced to their path; anything else (other origin, `//host`, `javascript:`)
 * is rejected.
 */
export function safeTargetUrl(url: unknown, origin: string): string | null {
	if (typeof url !== 'string' || url === '') return null;
	if (url.startsWith('//') || url.startsWith('/\\')) return null;
	let parsed: URL;
	try {
		parsed = new URL(url, origin);
	} catch {
		return null;
	}
	if (parsed.origin !== origin) return null;
	return `${parsed.pathname}${parsed.search}${parsed.hash}`;
}

/** Parses a push message body; anything but the expected string fields is dropped. */
export function parsePush(text: string | null): NotificationData | null {
	if (!text) return null;
	let raw: unknown;
	try {
		raw = JSON.parse(text);
	} catch {
		return null;
	}
	if (typeof raw !== 'object' || raw === null) return null;
	const r = raw as Record<string, unknown>;
	if (typeof r.title !== 'string' || !r.title) return null;
	return {
		title: r.title,
		body: typeof r.body === 'string' ? r.body : '',
		url: typeof r.url === 'string' ? r.url : '/work',
		tag: typeof r.tag === 'string' ? r.tag : 'enzarb'
	};
}

export type FetchPlan = 'asset' | 'navigate' | 'bypass';

/**
 * How the worker treats a request: static, hashed assets are served from the cache, navigations
 * only fall back to the offline page when the network fails, everything else (non-GET, remote
 * functions, `/live/`, cross-origin) is left to the browser.
 */
export function planFetch(
	req: { method: string; url: string; mode: string },
	origin: string,
	precached: ReadonlySet<string>
): FetchPlan {
	if (req.method !== 'GET') return 'bypass';
	const url = new URL(req.url);
	if (url.origin !== origin) return 'bypass';
	if (NEVER_CACHED.some((p) => url.pathname === p.replace(/\/$/, '') || url.pathname.startsWith(p)))
		return 'bypass';
	if (precached.has(url.pathname)) return 'asset';
	if (req.mode === 'navigate') return 'navigate';
	return 'bypass';
}

export interface WindowClientLike {
	url: string;
	focus(): Promise<unknown>;
	navigate?(url: string): Promise<unknown>;
}

export interface ClientsLike {
	matchAll(opts: {
		type: 'window';
		includeUncontrolled: boolean;
	}): Promise<readonly WindowClientLike[]>;
	openWindow(url: string): Promise<unknown>;
}

/**
 * Notification click: focus (and navigate) an open window of the app, else open one. A url that is
 * not same-origin falls back to the work queue.
 */
export async function openOrFocus(
	clients: ClientsLike,
	data: unknown,
	origin: string
): Promise<void> {
	const raw = (data as { url?: unknown } | null | undefined)?.url;
	const path = safeTargetUrl(raw, origin) ?? '/work';
	const windows = await clients.matchAll({ type: 'window', includeUncontrolled: true });
	const existing = windows.find((w) => new URL(w.url).origin === origin);
	if (existing) {
		await existing.focus();
		// navigate() is missing on some engines and rejects for uncontrolled clients: best effort,
		// the window is already in front.
		if (existing.navigate) await existing.navigate(path).catch(() => clients.openWindow(path));
		return;
	}
	await clients.openWindow(path);
}
