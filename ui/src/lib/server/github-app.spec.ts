import { generateKeyPairSync } from 'node:crypto';
import { decodeJwt, decodeProtectedHeader } from 'jose';
import { afterEach, describe, expect, it, vi } from 'vitest';

const cfg = vi.hoisted(() => ({ get: vi.fn() }));
vi.mock('./config.ts', () => ({ getConfig: cfg.get }));

import {
	createRepoTokenSource,
	githubFetch,
	invalidateToken,
	resetTokenSingleton,
	token
} from './github-app.ts';

const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
const pem = privateKey.export({ type: 'pkcs1', format: 'pem' }) as string;
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const NOW = Date.parse('2026-10-15T12:00:00Z');

function setup(now = () => NOW) {
	const f = vi.fn(async (url: string) =>
		url.endsWith('/installation')
			? json({ id: 9 })
			: json({ token: 'ghs_x', expires_at: new Date(NOW + 3600_000).toISOString() }, 201)
	);
	const src = createRepoTokenSource({
		appId: '1',
		privateKey: pem,
		fetch: f as unknown as typeof fetch,
		now
	});
	return { f, src };
}

describe('createRepoTokenSource', () => {
	it('looks up the repository installation and mints a down-scoped token', async () => {
		const { f, src } = setup();
		expect(await src.get('acme/widgets', { contents: 'write' })).toBe('ghs_x');
		expect(f.mock.calls[0][0]).toBe('https://api.github.com/repos/acme/widgets/installation');
		expect(f.mock.calls[1][0]).toBe('https://api.github.com/app/installations/9/access_tokens');
		expect(
			JSON.parse((f.mock.calls[1] as unknown as [string, RequestInit])[1].body as string)
		).toEqual({ repositories: ['widgets'], permissions: { contents: 'write' } });
	});

	it('caches tokens per permissions until shortly before expiry and dedupes mints', async () => {
		let t = NOW;
		const { f, src } = setup(() => t);
		await Promise.all([src.get('a/b', { contents: 'read' }), src.get('a/b', { contents: 'read' })]);
		expect(f).toHaveBeenCalledTimes(2);
		await src.get('a/b', { contents: 'read' });
		expect(f).toHaveBeenCalledTimes(2);
		// Other permissions: a new token, but the installation id is cached.
		await src.get('a/b', { contents: 'write' });
		expect(f).toHaveBeenCalledTimes(3);
		t = NOW + 3600_000 - 5 * 60_000;
		await src.get('a/b', { contents: 'read' });
		expect(f).toHaveBeenCalledTimes(4);
	});

	it('is null when unconfigured or not installed, and does not cache failures', async () => {
		const f = vi.fn(async () => json({}, 404));
		const opts = { appId: '1', privateKey: pem, fetch: f as unknown as typeof fetch };
		const src = createRepoTokenSource(opts);
		expect(await src.get('a/b', {})).toBeNull();
		expect(await src.get('a/b', {})).toBeNull();
		expect(f).toHaveBeenCalledTimes(2);
		expect(await createRepoTokenSource({ ...opts, appId: undefined }).get('a/b', {})).toBeNull();
	});
});

type Call = [string, RequestInit & { headers: Record<string, string> }];

describe('createRepoTokenSource details', () => {
	it('signs an RS256 JWT backdated 60s and sends the standard headers', async () => {
		const { f, src } = setup();
		await src.get('acme/widgets', { contents: 'read' });
		const [, lookup] = f.mock.calls[0] as unknown as Call;
		const [, mint] = f.mock.calls[1] as unknown as Call;
		expect(lookup.method).toBe('GET');
		expect(lookup.redirect).toBe('error');
		expect(mint.method).toBe('POST');
		expect(mint.headers).toMatchObject({
			Accept: 'application/vnd.github+json',
			'User-Agent': 'enzarb-ui'
		});
		const jwt = lookup.headers.Authorization.replace('Bearer ', '');
		expect(decodeProtectedHeader(jwt).alg).toBe('RS256');
		const claims = decodeJwt(jwt);
		expect(claims.iss).toBe('1');
		expect(claims.iat).toBe(NOW / 1000 - 60);
		expect(claims.exp).toBe(NOW / 1000 - 60 + 9 * 60);
	});

	it('refreshes exactly at expires_at minus the margin', async () => {
		let t = NOW;
		const { f, src } = setup(() => t);
		await src.get('a/b', { contents: 'read' });
		t = NOW + 3600_000 - 5 * 60_000 - 1;
		await src.get('a/b', { contents: 'read' });
		expect(f).toHaveBeenCalledTimes(2);
		t += 1;
		await src.get('a/b', { contents: 'read' });
		expect(f).toHaveBeenCalledTimes(3);
	});

	it('mints again after invalidate()', async () => {
		const { f, src } = setup();
		await src.get('a/b', { contents: 'read' });
		src.invalidate('A/b', { contents: 'read' });
		await src.get('a/b', { contents: 'read' });
		expect(f).toHaveBeenCalledTimes(3);
	});

	it('shares one in-flight mint', async () => {
		const { f, src } = setup();
		const out = await Promise.all([1, 2, 3].map(() => src.get('a/b', { contents: 'read' })));
		expect(out).toEqual(['ghs_x', 'ghs_x', 'ghs_x']);
		expect(f).toHaveBeenCalledTimes(2);
	});

	it('is null on a mint error and does not cache it', async () => {
		let fail = true;
		const f = vi.fn(async (url: string) =>
			url.endsWith('/installation')
				? json({ id: 9 })
				: fail
					? json({}, 500)
					: json({ token: 'ghs_x', expires_at: new Date(NOW + 3600_000).toISOString() }, 201)
		);
		const src = createRepoTokenSource({
			appId: '1',
			privateKey: pem,
			fetch: f as unknown as typeof fetch,
			now: () => NOW
		});
		expect(await src.get('a/b', {})).toBeNull();
		fail = false;
		expect(await src.get('a/b', {})).toBe('ghs_x');
	});

	it('never calls GitHub without credentials', async () => {
		const f = vi.fn();
		const src = createRepoTokenSource({ fetch: f as unknown as typeof fetch });
		expect(await src.get('a/b', {})).toBeNull();
		expect(f).not.toHaveBeenCalled();
	});

	it('resolves null (no throw, no key material) for a bad key or a network error', async () => {
		const bad = createRepoTokenSource({
			appId: '1',
			privateKey: '-----BEGIN RSA PRIVATE KEY-----\nSECRETBITS\n-----END RSA PRIVATE KEY-----',
			fetch: vi.fn() as unknown as typeof fetch
		});
		await expect(bad.get('a/b', {})).resolves.toBeNull();
		const down = createRepoTokenSource({
			appId: '1',
			privateKey: pem,
			fetch: vi.fn(async () => {
				throw new Error('socket hang up');
			}) as unknown as typeof fetch
		});
		await expect(down.get('a/b', {})).resolves.toBeNull();
	});
});

describe('token singleton', () => {
	afterEach(() => {
		resetTokenSingleton();
		vi.unstubAllGlobals();
		cfg.get.mockReset();
	});

	it('reads config lazily and reuses one cache', async () => {
		const f = vi.fn(async (url: string) =>
			url.endsWith('/installation')
				? json({ id: 9 })
				: json({ token: 'ghs_x', expires_at: new Date(Date.now() + 3600_000).toISOString() }, 201)
		);
		vi.stubGlobal('fetch', f);
		expect(cfg.get).not.toHaveBeenCalled();
		cfg.get.mockReturnValue({ githubAppId: '1', githubAppPrivateKey: pem });
		expect(await token('a/b', { contents: 'read' })).toBe('ghs_x');
		expect(await token('a/b', { contents: 'read' })).toBe('ghs_x');
		expect(cfg.get).toHaveBeenCalledTimes(1);
		expect(f).toHaveBeenCalledTimes(2);
		invalidateToken('a/b', { contents: 'read' });
		await token('a/b', { contents: 'read' });
		expect(f).toHaveBeenCalledTimes(3);
	});
});

describe('githubFetch', () => {
	const ok = () => vi.fn(async () => json({}));
	const call = (f: ReturnType<typeof ok>) => f.mock.calls[0] as unknown as Call;

	it('prefixes the base URL and sets the standard headers without auth', async () => {
		const f = ok();
		await githubFetch('/user', { fetch: f as unknown as typeof fetch, redirect: 'error' });
		const [url, init] = call(f);
		expect(url).toBe('https://api.github.com/user');
		expect(init.redirect).toBe('error');
		expect(init.headers).toMatchObject({
			Accept: 'application/vnd.github+json',
			'User-Agent': 'enzarb-ui',
			'X-GitHub-Api-Version': '2022-11-28'
		});
		expect(init.headers.Authorization).toBeUndefined();
	});

	it('adds a Bearer token, lets init.headers override and accepts API URLs', async () => {
		const f = ok();
		await githubFetch('https://api.github.com/x?page=2', {
			token: 't',
			headers: { Accept: 'text/plain' },
			fetch: f as unknown as typeof fetch
		});
		const [url, init] = call(f);
		expect(url).toBe('https://api.github.com/x?page=2');
		expect(init.headers).toMatchObject({ Authorization: 'Bearer t', Accept: 'text/plain' });
	});

	it('rejects foreign URLs without calling fetch', () => {
		const f = ok();
		for (const u of ['https://evil.example/x', 'https://api.github.com.evil.example/x', 'user'])
			expect(() => githubFetch(u, { token: 't', fetch: f as unknown as typeof fetch })).toThrow(
				'unexpected GitHub URL'
			);
		expect(f).not.toHaveBeenCalled();
	});
});
