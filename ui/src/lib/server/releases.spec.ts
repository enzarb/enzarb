import { describe, expect, it, vi } from 'vitest';
import { createReleaseCache, fetchReleases, type Release } from './releases.ts';

const gh = (over: Record<string, unknown> = {}) => ({
	tag_name: 'v1.0.0',
	name: 'One',
	published_at: '2026-10-01T00:00:00Z',
	html_url: 'https://github.com/enzarb/enzarb-ui/releases/tag/v1.0.0',
	body: 'notes',
	draft: false,
	...over
});
const respond = (body: unknown, status = 200) =>
	vi.fn(async () => new Response(JSON.stringify(body), { status }));

describe('fetchReleases', () => {
	it('maps releases, drops drafts and sends the GitHub headers', async () => {
		const f = respond([gh(), gh({ tag_name: 'v0.9.0', draft: true })]);
		const out = await fetchReleases('enzarb-ui', { fetch: f as unknown as typeof fetch });
		expect(out).toEqual([
			{
				tag: 'v1.0.0',
				name: 'One',
				publishedAt: '2026-10-01T00:00:00Z',
				url: 'https://github.com/enzarb/enzarb-ui/releases/tag/v1.0.0',
				body: 'notes'
			}
		]);
		const [url, init] = f.mock.calls[0] as unknown as [string, RequestInit];
		expect(url).toBe('https://api.github.com/repos/enzarb/enzarb-ui/releases?per_page=30');
		expect(init.headers).toMatchObject({
			Accept: 'application/vnd.github+json',
			'User-Agent': 'enzarb-ui'
		});
		expect(init.signal).toBeInstanceOf(AbortSignal);
	});
	it('sends a token only when given', async () => {
		const f = respond([]);
		await fetchReleases('x', { fetch: f as unknown as typeof fetch, token: 't' });
		expect((f.mock.calls[0] as unknown as [string, RequestInit])[1].headers).toMatchObject({
			Authorization: 'Bearer t'
		});
	});
	it('throws on a non-OK response (rate limit) and on a non-array body', async () => {
		await expect(
			fetchReleases('x', { fetch: respond({ message: 'rate' }, 403) as unknown as typeof fetch })
		).rejects.toThrow(/HTTP 403/);
		await expect(
			fetchReleases('x', { fetch: respond({}) as unknown as typeof fetch })
		).rejects.toThrow(/unexpected/);
	});
	it('aborts on timeout', async () => {
		const f = vi.fn(
			(_url: string, init: RequestInit) =>
				new Promise<Response>((_, reject) => {
					init.signal?.addEventListener('abort', () => reject(init.signal?.reason));
				})
		);
		await expect(
			fetchReleases('x', { fetch: f as unknown as typeof fetch, signal: AbortSignal.timeout(10) })
		).rejects.toBeDefined();
	});
});

describe('createReleaseCache', () => {
	const rel: Release[] = [{ tag: 'v1', name: 'v1', publishedAt: null, url: '', body: '' }];

	it('serves a hit until the TTL expires', async () => {
		let t = 0;
		const cache = createReleaseCache(1000, () => t);
		const load = vi.fn(async () => rel);
		await cache.get('a', load);
		t = 999;
		await cache.get('a', load);
		expect(load).toHaveBeenCalledTimes(1);
		t = 1000;
		await cache.get('a', load);
		expect(load).toHaveBeenCalledTimes(2);
	});
	it('caches per repo', async () => {
		const cache = createReleaseCache();
		const load = vi.fn(async () => rel);
		await cache.get('a', load);
		await cache.get('b', load);
		expect(load).toHaveBeenCalledTimes(2);
	});
	it('shares one in-flight request between concurrent callers', async () => {
		const cache = createReleaseCache();
		const load = vi.fn(async () => rel);
		await Promise.all([cache.get('a', load), cache.get('a', load)]);
		expect(load).toHaveBeenCalledTimes(1);
	});
	it('does not cache failures', async () => {
		const cache = createReleaseCache();
		const load = vi
			.fn<() => Promise<Release[]>>()
			.mockRejectedValueOnce(new Error('boom'))
			.mockResolvedValueOnce(rel);
		await expect(cache.get('a', load)).rejects.toThrow('boom');
		await expect(cache.get('a', load)).resolves.toEqual(rel);
		expect(load).toHaveBeenCalledTimes(2);
	});
});
