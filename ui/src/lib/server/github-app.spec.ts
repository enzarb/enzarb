import { generateKeyPairSync } from 'node:crypto';
import { describe, expect, it, vi } from 'vitest';
import { createRepoTokenSource } from './github-app.ts';

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
