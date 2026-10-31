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

describe('review reads', () => {
	it('reads files with patches (binary files have none) and caches them', async () => {
		const { reads, f } = setup(async () =>
			json([
				{
					filename: 'a.ts',
					status: 'modified',
					additions: 1,
					deletions: 0,
					patch: '@@ -1 +1 @@\n+x'
				},
				{ filename: 'b.png', status: 'added', additions: 0, deletions: 0 }
			])
		);
		const r = await reads.pullFiles('o/r', 5);
		expect(r.items.map((x) => [x.filename, x.patch !== undefined])).toEqual([
			['a.ts', true],
			['b.png', false]
		]);
		await reads.pullFiles('o/r', 5);
		expect(f).toHaveBeenCalledTimes(1);
	});

	it('reads check runs from the wrapper object, and fresh skips the cache', async () => {
		const { reads, f } = setup(async () =>
			json({
				total_count: 1,
				check_runs: [{ name: 'build', status: 'completed', conclusion: 'success', html_url: 'u' }]
			})
		);
		expect((await reads.checkRuns('o/r', 'sha')).items).toEqual([
			{ name: 'build', status: 'completed', conclusion: 'success', url: 'u' }
		]);
		await reads.checkRuns('o/r', 'sha');
		expect(f).toHaveBeenCalledTimes(1);
		await reads.checkRuns('o/r', 'sha', { fresh: true });
		expect(f).toHaveBeenCalledTimes(2);
	});

	it('reads one pull request and only comments with the ez-tracks marker', async () => {
		const one = setup(async () =>
			json({
				number: 5,
				title: 'T',
				html_url: 'u',
				draft: false,
				head: { sha: 'abc' },
				mergeable: null,
				mergeable_state: 'unknown'
			})
		);
		expect((await one.reads.pull('o/r', 5)).pull).toMatchObject({
			headSha: 'abc',
			mergeable: null
		});
		const c = setup(async () => json([{ body: 'hi' }, { body: 'x <!-- ez-tracks:r:n:1 -->' }]));
		expect((await c.reads.markedComments('o/r', 5)).items).toEqual(['x <!-- ez-tracks:r:n:1 -->']);
	});
});

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

	it('cuts long issue bodies and says so', async () => {
		const { reads } = setup(async () =>
			json([
				{ number: 1, title: 'a', body: 'x'.repeat(5000), html_url: 'u', updated_at: 't' },
				{ number: 2, title: 'b', body: 'short', html_url: 'u', updated_at: 't' }
			])
		);
		const [long, short] = (await reads.issues('o/r', 'triage')).items;
		expect(long.body).toHaveLength(4000);
		expect(long.bodyTruncated).toBe(true);
		expect(short.bodyTruncated).toBe(false);
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
