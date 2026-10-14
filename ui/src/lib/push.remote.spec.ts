import { isHttpError } from '@sveltejs/kit';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const locals = vi.hoisted(() => ({ user: undefined as unknown }));
const store = vi.hoisted(() => ({
	list: vi.fn(),
	upsert: vi.fn(),
	setCategories: vi.fn(),
	remove: vi.fn(),
	targets: vi.fn(),
	touch: vi.fn()
}));
const send = vi.hoisted(() => vi.fn());
const vapid = vi.hoisted(() => ({ key: 'PUBLIC' as string | null }));
vi.mock('$app/server', () => ({
	// remote functions: (schema, handler) or (handler); the handler runs as-is
	command: (a: object, b?: object) => Object.assign(b ?? a, { __: { type: 'command' } }),
	query: (a: object, b?: object) => Object.assign(b ?? a, { __: { type: 'query' } }),
	getRequestEvent: () => ({ locals, request: { headers: new Headers({ 'user-agent': 'UA/1' }) } })
}));
vi.mock('#lib/server/push.ts', () => ({
	getPushStore: () => store,
	getSend: () => send,
	vapidPublicKey: () => vapid.key,
	deliver: async (_s: unknown, _send: unknown, _o: unknown, targets: unknown[]) => targets.length
}));

import { getPushState, sendTest, subscribe, unsubscribe, updateCategories } from './push.remote.ts';

type Fn = (...args: unknown[]) => Promise<unknown>;
const call = (fn: unknown, ...args: unknown[]) => (fn as Fn)(...args);
const sub = {
	endpoint: 'https://push.example/abc',
	keys: { p256dh: 'k', auth: 'a' },
	categories: ['failed']
};

beforeEach(() => {
	for (const f of Object.values(store)) f.mockReset();
	store.list.mockResolvedValue([]);
	store.upsert.mockResolvedValue(true);
	store.setCategories.mockResolvedValue(true);
	store.targets.mockResolvedValue([{ endpoint: sub.endpoint, p256dh: 'k', auth: 'a' }]);
	vapid.key = 'PUBLIC';
	locals.user = { id: 1, login: 'octo', name: null };
});

describe('authz', () => {
	it('answers 401 and touches nothing when not signed in', async () => {
		locals.user = undefined;
		for (const run of [
			() => call(getPushState),
			() => call(subscribe, sub),
			() => call(updateCategories, { endpoint: sub.endpoint, categories: [] }),
			() => call(unsubscribe, sub.endpoint),
			() => call(sendTest, sub.endpoint)
		]) {
			const err = await run().catch((e) => e);
			expect(isHttpError(err, 401)).toBe(true);
		}
		for (const f of Object.values(store)) expect(f).not.toHaveBeenCalled();
	});

	it('always uses the session login as the owner', async () => {
		await call(subscribe, sub);
		expect(store.upsert).toHaveBeenCalledWith(
			'octo',
			expect.objectContaining({ endpoint: sub.endpoint, userAgent: 'UA/1' })
		);
		await call(updateCategories, { endpoint: sub.endpoint, categories: ['merge'] });
		expect(store.setCategories).toHaveBeenCalledWith('octo', sub.endpoint, ['merge']);
		await call(unsubscribe, sub.endpoint);
		expect(store.remove).toHaveBeenCalledWith('octo', sub.endpoint);
		await call(getPushState);
		expect(store.list).toHaveBeenCalledWith('octo');
	});

	it('rejects an endpoint that belongs to someone else without saying so', async () => {
		store.upsert.mockResolvedValue(false);
		const err = await call(subscribe, sub).catch((e) => e);
		expect(isHttpError(err, 409)).toBe(true);
		expect(String((err as { body: { message: string } }).body.message)).not.toMatch(
			/owner|another|belongs/i
		);
	});

	it('is 404 when updating a device that is not yours', async () => {
		store.setCategories.mockResolvedValue(false);
		const err = await call(updateCategories, { endpoint: sub.endpoint, categories: [] }).catch(
			(e) => e
		);
		expect(isHttpError(err, 404)).toBe(true);
	});
});

describe('getPushState', () => {
	it('returns the public key and the devices, nothing secret', async () => {
		store.list.mockResolvedValue([
			{ endpoint: sub.endpoint, categories: ['failed'], userAgent: null, createdAt: 'x' }
		]);
		const state = (await call(getPushState)) as { publicKey: string; devices: object[] };
		expect(state.publicKey).toBe('PUBLIC');
		expect(JSON.stringify(state)).not.toMatch(/p256dh|auth"|private/i);
	});
});

describe('subscribe', () => {
	it('is 503 without VAPID keys', async () => {
		vapid.key = null;
		const err = await call(subscribe, sub).catch((e) => e);
		expect(isHttpError(err, 503)).toBe(true);
		expect(store.upsert).not.toHaveBeenCalled();
	});
});

describe('sendTest', () => {
	it('only sends to one of the caller devices', async () => {
		const err = await call(sendTest, 'https://push.example/other').catch((e) => e);
		expect(isHttpError(err, 404)).toBe(true);
	});

	it('is rate limited per user', async () => {
		locals.user = { id: 2, login: 'rate-limited', name: null };
		await call(sendTest, sub.endpoint);
		const err = await call(sendTest, sub.endpoint).catch((e) => e);
		expect(isHttpError(err, 429)).toBe(true);
	});
});
