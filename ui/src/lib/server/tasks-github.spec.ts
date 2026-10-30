import { describe, expect, it, vi } from 'vitest';
import { createGithubReads, rateLimitWait, TTL_MS } from './tasks-github.ts';

const json = (body: unknown, init: ResponseInit = {}) =>
	new Response(JSON.stringify(body), {
		status: 200,
		headers: { 'content-type': 'application/json' },
		...init
	});

const pr = (n: number, ref: string) => ({
	number: n,
	title: `PR ${n}`,
	html_url: `https://github.com/o/r/pull/${n}`,
	head: { ref },
	draft: false,
	updated_at: '2026-10-30T00:00:00Z'
});

function setup(fetchImpl: () => Promise<Response>) {
	let t = 1_000_000;
	const f = vi.fn(fetchImpl);
	const reads = createGithubReads({
		token: async () => 'tok',
		invalidate: () => {},
		fetch: f as unknown as typeof fetch,
		now: () => t
	});
	return { reads, f, advance: (ms: number) => (t += ms) };
}

describe('tasks GitHub reads', () => {
	it('keeps only ez/* pull requests and caches per repository', async () => {
		const { reads, f, advance } = setup(async () =>
			json([pr(1, 'ez/fix/abc'), pr(2, 'feature/x')])
		);
		const a = await reads.pulls('o/r');
		expect(a.items.map((p) => p.number)).toEqual([1]);
		await Promise.all([reads.pulls('o/r'), reads.pulls('o/r')]);
		expect(f).toHaveBeenCalledTimes(1);
		advance(TTL_MS + 1);
		await reads.pulls('o/r');
		expect(f).toHaveBeenCalledTimes(2);
	});

	it('reports open subjects, or unknown when the list was cut off', async () => {
		const { reads } = setup(async () => json([{ number: 7 }, { number: 9 }]));
		expect([...((await reads.openSubjects('o/r')).open ?? [])]).toEqual([7, 9]);

		// Every page links to a next one: more than MAX_PAGES, so the open set is incomplete.
		const many = setup(async () =>
			json([{ number: 1 }], {
				headers: {
					'content-type': 'application/json',
					link: '<https://api.github.com/repos/o/r/issues?page=2>; rel="next"'
				}
			})
		);
		expect((await many.reads.openSubjects('o/r')).open).toBeNull();
	});

	it('shares one request between concurrent callers', async () => {
		const { reads, f } = setup(async () => json([pr(1, 'ez/a/b')]));
		await Promise.all([reads.pulls('o/r'), reads.pulls('o/r'), reads.pulls('o/r')]);
		expect(f).toHaveBeenCalledTimes(1);
	});

	it('drops pull requests from the issues list', async () => {
		const { reads, f } = setup(async () =>
			json([
				{ number: 1, title: 'a', html_url: 'u', updated_at: 't' },
				{ number: 2, title: 'b', html_url: 'u', updated_at: 't', pull_request: {} }
			])
		);
		expect((await reads.issues('o/r', 'triage')).items.map((i) => i.number)).toEqual([1]);
		expect((f.mock.calls[0] as unknown as [string])[0]).toContain('labels=triage');
	});

	it('backs off after a rate limit and serves the last good answer', async () => {
		let limited = false;
		const { reads, f, advance } = setup(async () =>
			limited
				? new Response('rate limit', { status: 403, headers: { 'retry-after': '120' } })
				: json([pr(1, 'ez/a/b')])
		);
		await reads.pulls('o/r');
		advance(TTL_MS + 1);
		limited = true;
		const r = await reads.pulls('o/r');
		expect(r.items.map((p) => p.number)).toEqual([1]);
		expect(r.note).toContain('rate limit');
		// Nothing is sent while the limit is known to be active (even for another repository).
		const other = await reads.pulls('o/other');
		expect(other.note).toContain('rate limit');
		expect(f).toHaveBeenCalledTimes(2);
		advance(121_000);
		limited = false;
		expect((await reads.pulls('o/other')).note).toBeUndefined();
	});

	it('reports an App without access', async () => {
		const reads = createGithubReads({ token: async () => null, invalidate: () => {} });
		expect((await reads.pulls('o/r')).note).toContain('cannot read');
	});

	it('tells rate limits from permission errors', () => {
		expect(rateLimitWait(new Response('', { status: 403 }), 0)).toBeNull();
		expect(rateLimitWait(new Response('', { status: 429 }), 0)).toBe(60_000);
		expect(
			rateLimitWait(
				new Response('', {
					status: 403,
					headers: { 'x-ratelimit-remaining': '0', 'x-ratelimit-reset': '100' }
				}),
				90_000
			)
		).toBe(10_000);
	});
});
