import { generateKeyPairSync } from 'node:crypto';
import { decodeJwt, decodeProtectedHeader } from 'jose';
import { describe, expect, it, vi } from 'vitest';
import { createInstallationTokenSource } from './github-app-token.ts';

const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
const pem = privateKey.export({ type: 'pkcs1', format: 'pem' }) as string;

const NOW = Date.parse('2026-10-13T12:00:00Z');
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const expiresIn = (ms: number) => new Date(NOW + ms).toISOString();

function setup(over: { install?: Response; mint?: () => Response; now?: () => number } = {}) {
	const f = vi.fn(async (url: string) =>
		url.endsWith('/installation')
			? (over.install ?? json({ id: 42 }))
			: (over.mint?.() ?? json({ token: 'ghs_tok', expires_at: expiresIn(3600_000) }, 201))
	);
	const src = createInstallationTokenSource({
		appId: '123',
		privateKey: pem,
		fetch: f as unknown as typeof fetch,
		now: over.now ?? (() => NOW)
	});
	return { f, src };
}
const init = (f: ReturnType<typeof setup>['f'], i: number) =>
	f.mock.calls[i] as unknown as [string, RequestInit & { headers: Record<string, string> }];

describe('installation token source', () => {
	it('signs an RS256 app JWT, looks up the installation and mints a token', async () => {
		const { f, src } = setup();
		expect(await src.get()).toBe('ghs_tok');
		const [lookupUrl, lookup] = init(f, 0);
		expect(lookupUrl).toBe('https://api.github.com/repos/enzarb/enzarb/installation');
		expect(lookup.method).toBe('GET');
		const [mintUrl, mint] = init(f, 1);
		expect(mintUrl).toBe('https://api.github.com/app/installations/42/access_tokens');
		expect(mint.method).toBe('POST');
		expect(mint.headers).toMatchObject({
			Accept: 'application/vnd.github+json',
			'User-Agent': 'enzarb-ui'
		});
		const jwt = lookup.headers.Authorization.replace('Bearer ', '');
		expect(decodeProtectedHeader(jwt).alg).toBe('RS256');
		const claims = decodeJwt(jwt);
		expect(claims.iss).toBe('123');
		expect(claims.iat).toBe(NOW / 1000 - 60);
		expect(claims.exp).toBe(NOW / 1000 - 60 + 9 * 60);
	});

	it('reuses the cached token and refreshes it shortly before expiry', async () => {
		let t = NOW;
		const { f, src } = setup({ now: () => t });
		await src.get();
		t = NOW + 3600_000 - 5 * 60_000 - 1;
		await src.get();
		expect(f).toHaveBeenCalledTimes(2);
		t = NOW + 3600_000 - 5 * 60_000;
		await src.get();
		expect(f).toHaveBeenCalledTimes(4);
	});

	it('mints once for concurrent callers', async () => {
		const { f, src } = setup();
		const out = await Promise.all([src.get(), src.get(), src.get()]);
		expect(out).toEqual(['ghs_tok', 'ghs_tok', 'ghs_tok']);
		expect(f).toHaveBeenCalledTimes(2);
	});

	it('mints again after invalidate()', async () => {
		const { f, src } = setup();
		await src.get();
		src.invalidate();
		await src.get();
		expect(f).toHaveBeenCalledTimes(4);
	});

	it('returns null when the App is not installed (404)', async () => {
		const { f, src } = setup({ install: json({ message: 'Not Found' }, 404) });
		expect(await src.get()).toBeNull();
		expect(f).toHaveBeenCalledTimes(1);
	});

	it('returns null on a mint error and does not cache the failure', async () => {
		let fail = true;
		const { f, src } = setup({
			mint: () =>
				fail
					? json({ message: 'nope' }, 500)
					: json({ token: 'ghs_tok', expires_at: expiresIn(3600_000) }, 201)
		});
		expect(await src.get()).toBeNull();
		fail = false;
		expect(await src.get()).toBe('ghs_tok');
		expect(f).toHaveBeenCalledTimes(4);
	});

	it('returns null without credentials and never calls GitHub', async () => {
		const f = vi.fn();
		const src = createInstallationTokenSource({ fetch: f as unknown as typeof fetch });
		expect(await src.get()).toBeNull();
		expect(f).not.toHaveBeenCalled();
	});

	it('resolves null (no throw, no key material) for a bad key or a network error', async () => {
		const bad = createInstallationTokenSource({
			appId: '1',
			privateKey: '-----BEGIN RSA PRIVATE KEY-----\nSECRETBITS\n-----END RSA PRIVATE KEY-----',
			fetch: vi.fn() as unknown as typeof fetch
		});
		await expect(bad.get()).resolves.toBeNull();
		const down = createInstallationTokenSource({
			appId: '1',
			privateKey: pem,
			fetch: vi.fn(async () => {
				throw new Error('socket hang up');
			}) as unknown as typeof fetch
		});
		await expect(down.get()).resolves.toBeNull();
	});
});
