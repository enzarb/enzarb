import { describe, expect, it, vi } from 'vitest';
import {
	handlePush,
	OFFLINE_PATH,
	openOrFocus,
	parsePush,
	planFetch,
	safeTargetUrl,
	type ClientsLike,
	type WindowClientLike
} from './sw-handlers.ts';

const ORIGIN = 'https://enzarb.example.com';

function fakeClients(windows: WindowClientLike[]) {
	const clients = {
		matchAll: vi.fn(async () => windows),
		openWindow: vi.fn(async () => null)
	} satisfies ClientsLike;
	return clients;
}

const win = (url: string, navigate = vi.fn(async () => null)) => ({
	url,
	focus: vi.fn(async () => null),
	navigate
});

describe('safeTargetUrl', () => {
	it('keeps in-app paths', () => {
		expect(safeTargetUrl('/work/3', ORIGIN)).toBe('/work/3');
		expect(safeTargetUrl('/work?x=1#a', ORIGIN)).toBe('/work?x=1#a');
		expect(safeTargetUrl(`${ORIGIN}/work/3`, ORIGIN)).toBe('/work/3');
	});

	it('rejects other origins and non-urls', () => {
		for (const u of [
			'https://evil.example/work/3',
			'//evil.example/work',
			'/\\evil.example',
			'javascript:alert(1)',
			'',
			undefined,
			42
		]) {
			expect(safeTargetUrl(u, ORIGIN), String(u)).toBeNull();
		}
	});
});

describe('openOrFocus (notificationclick)', () => {
	it('focuses and navigates an existing window of the app', async () => {
		const w = win(`${ORIGIN}/projects`);
		const clients = fakeClients([win('https://other.example/'), w]);
		await openOrFocus(clients, { url: '/work/9' }, ORIGIN);
		expect(w.focus).toHaveBeenCalledOnce();
		expect(w.navigate).toHaveBeenCalledWith('/work/9');
		expect(clients.openWindow).not.toHaveBeenCalled();
	});

	it('opens a window when none is open', async () => {
		const clients = fakeClients([win('https://other.example/')]);
		await openOrFocus(clients, { url: '/work/9' }, ORIGIN);
		expect(clients.openWindow).toHaveBeenCalledExactlyOnceWith('/work/9');
	});

	it('never opens a cross-origin url from the payload', async () => {
		const clients = fakeClients([]);
		await openOrFocus(clients, { url: 'https://evil.example/phish' }, ORIGIN);
		expect(clients.openWindow).toHaveBeenCalledExactlyOnceWith('/work');
		await openOrFocus(clients, undefined, ORIGIN);
		expect(clients.openWindow).toHaveBeenLastCalledWith('/work');
	});

	it('opens a window when navigating the existing one fails', async () => {
		const w = win(
			`${ORIGIN}/`,
			vi.fn(async () => Promise.reject(new Error('no')))
		);
		const clients = fakeClients([w]);
		await openOrFocus(clients, { url: '/work/1' }, ORIGIN);
		expect(w.focus).toHaveBeenCalled();
		expect(clients.openWindow).toHaveBeenCalledWith('/work/1');
	});
});

describe('planFetch', () => {
	const precached = new Set(['/_app/immutable/a.js', OFFLINE_PATH, '/manifest.webmanifest']);
	const plan = (path: string, method = 'GET', mode = 'cors', origin = ORIGIN) =>
		planFetch({ method, url: `${origin}${path}`, mode }, ORIGIN, precached);

	it('serves precached static files from the cache', () => {
		expect(plan('/_app/immutable/a.js')).toBe('asset');
		expect(plan('/manifest.webmanifest')).toBe('asset');
	});

	it('only falls back to the offline page for navigations', () => {
		expect(plan('/work', 'GET', 'navigate')).toBe('navigate');
		expect(plan('/work/3', 'GET', 'cors')).toBe('bypass');
	});

	it('never touches remote functions, the realtime stream or auth routes', () => {
		for (const p of [
			'/_app/remote/abc/getTask',
			'/live/',
			'/live/ws',
			'/login/github',
			'/logout',
			'/sandbox/v1/projects/p/mux'
		]) {
			expect(plan(p, 'GET', 'navigate'), p).toBe('bypass');
			expect(plan(p, 'GET', 'cors'), p).toBe('bypass');
		}
	});

	it('ignores non-GET and cross-origin requests', () => {
		expect(plan('/_app/immutable/a.js', 'POST')).toBe('bypass');
		expect(plan('/work', 'PUT', 'navigate')).toBe('bypass');
		expect(plan('/_app/immutable/a.js', 'GET', 'cors', 'https://cdn.example')).toBe('bypass');
	});
});

describe('parsePush', () => {
	it('parses a payload and defaults the optional fields', () => {
		expect(parsePush('{"title":"T","body":"B","url":"/work/1","tag":"work-1"}')).toEqual({
			title: 'T',
			body: 'B',
			url: '/work/1',
			tag: 'work-1'
		});
		expect(parsePush('{"title":"T"}')).toEqual({
			title: 'T',
			body: '',
			url: '/work',
			tag: 'enzarb'
		});
	});

	it('drops anything unusable', () => {
		for (const t of [null, '', 'nope', '[]', '{"body":"x"}', '{"title":3}']) {
			expect(parsePush(t), String(t)).toBeNull();
		}
	});

	it('parses a clear request, and drops one without a tag', () => {
		expect(parsePush('{"type":"clear","tag":"item-3"}')).toEqual({ type: 'clear', tag: 'item-3' });
		expect(parsePush('{"type":"clear"}')).toBeNull();
	});
});

describe('handlePush', () => {
	const reg = (open: string[]) => {
		const closed: string[] = [];
		return {
			closed,
			getNotifications: vi.fn(async ({ tag }: { tag: string }) =>
				open.filter((t) => t === tag).map((t) => ({ close: () => closed.push(t) }))
			),
			showNotification: vi.fn(async () => {})
		};
	};

	it('shows a notification with its tag and url', async () => {
		const r = reg([]);
		await handlePush(r, { title: 'T', body: 'B', url: '/work?highlight=1', tag: 'item-1' });
		expect(r.showNotification).toHaveBeenCalledWith(
			'T',
			expect.objectContaining({ tag: 'item-1', data: { url: '/work?highlight=1' } })
		);
	});

	it('closes the matching notifications on a clear and shows nothing', async () => {
		const r = reg(['item-1', 'item-2']);
		await handlePush(r, { type: 'clear', tag: 'item-1' });
		expect(r.closed).toEqual(['item-1']);
		expect(r.showNotification).not.toHaveBeenCalled();
	});

	it('does nothing visible when there is nothing to clear', async () => {
		const r = reg([]);
		await handlePush(r, { type: 'clear', tag: 'item-9' });
		await handlePush(r, null);
		expect(r.showNotification).not.toHaveBeenCalled();
	});
});
