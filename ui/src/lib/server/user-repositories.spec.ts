import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import { migrate } from '../../../scripts/migrate.ts';
import { upsertUser } from './auth.ts';
import {
	fetchUserRepositories,
	getUserRepositorySnapshot,
	hasUserRepositories,
	installUrl,
	listUserRepositories,
	syncUserRepositories
} from './user-repositories.ts';

const API = 'https://api.github.com';
const json = (body: unknown, link?: string) =>
	new Response(JSON.stringify(body), {
		status: 200,
		headers: { 'content-type': 'application/json', ...(link ? { link } : {}) }
	});
const repo = (full_name: string, priv = false) => ({
	full_name,
	private: priv,
	default_branch: 'main'
});

/** A fake GitHub keyed by URL; records every request. */
function fakeGitHub(routes: Record<string, () => Response>) {
	const calls: { url: string; headers: Record<string, string> }[] = [];
	const fn = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
		const url = String(input);
		calls.push({ url, headers: init?.headers as Record<string, string> });
		const route = routes[url];
		return route ? route() : new Response('not found', { status: 404 });
	}) as unknown as typeof fetch;
	return { fn, calls };
}

describe('fetchUserRepositories', () => {
	it('returns nothing without installations', async () => {
		const gh = fakeGitHub({
			[`${API}/user/installations?per_page=100`]: () => json({ total_count: 0, installations: [] })
		});
		expect(await fetchUserRepositories('tok', gh.fn)).toEqual([]);
	});

	it('merges several installations, follows pagination and lowercases keys', async () => {
		const gh = fakeGitHub({
			[`${API}/user/installations?per_page=100`]: () =>
				json({
					installations: [
						{ id: 1, account: { login: 'Alice' } },
						{ id: 2, account: { login: 'Acme' } }
					]
				}),
			[`${API}/user/installations/1/repositories?per_page=100`]: () =>
				json(
					{ repositories: [repo('Alice/One', true)] },
					`<${API}/user/installations/1/repositories?per_page=100&page=2>; rel="next", <${API}/x>; rel="last"`
				),
			[`${API}/user/installations/1/repositories?per_page=100&page=2`]: () =>
				json({ repositories: [repo('Alice/Two')] }),
			[`${API}/user/installations/2/repositories?per_page=100`]: () =>
				json({ repositories: [repo('Acme/Svc')] })
		});
		const repos = await fetchUserRepositories('tok', gh.fn);
		expect(repos).toEqual([
			{
				repo: 'github:alice/one',
				installationId: 1,
				accountLogin: 'Alice',
				private: true,
				defaultBranch: 'main'
			},
			{
				repo: 'github:alice/two',
				installationId: 1,
				accountLogin: 'Alice',
				private: false,
				defaultBranch: 'main'
			},
			{
				repo: 'github:acme/svc',
				installationId: 2,
				accountLogin: 'Acme',
				private: false,
				defaultBranch: 'main'
			}
		]);
		for (const c of gh.calls) {
			expect(c.headers).toMatchObject({
				Authorization: 'Bearer tok',
				Accept: 'application/vnd.github+json',
				'User-Agent': 'enzarb-ui'
			});
		}
	});

	it('paginates the installations list too', async () => {
		const gh = fakeGitHub({
			[`${API}/user/installations?per_page=100`]: () =>
				json(
					{ installations: [{ id: 1, account: { login: 'a' } }] },
					`<${API}/user/installations?per_page=100&page=2>; rel="next"`
				),
			[`${API}/user/installations?per_page=100&page=2`]: () =>
				json({ installations: [{ id: 2, account: { login: 'b' } }] }),
			[`${API}/user/installations/1/repositories?per_page=100`]: () =>
				json({ repositories: [repo('a/x')] }),
			[`${API}/user/installations/2/repositories?per_page=100`]: () =>
				json({ repositories: [repo('b/y')] })
		});
		expect((await fetchUserRepositories('t', gh.fn)).map((r) => r.repo)).toEqual([
			'github:a/x',
			'github:b/y'
		]);
	});

	it('refuses to send the token to a foreign pagination URL', async () => {
		const gh = fakeGitHub({
			[`${API}/user/installations?per_page=100`]: () =>
				json({ installations: [] }, '<https://evil.example/next>; rel="next"')
		});
		await expect(fetchUserRepositories('tok', gh.fn)).rejects.toThrow(/unexpected pagination/);
		expect(gh.calls).toHaveLength(1);
	});

	it('throws on an API error', async () => {
		const gh = fakeGitHub({
			[`${API}/user/installations?per_page=100`]: () => new Response('no', { status: 500 })
		});
		await expect(fetchUserRepositories('tok', gh.fn)).rejects.toThrow(/500/);
	});
});

describe('fetchUserRepositories edge cases', () => {
	it('skips suspended installations and treats a 404 as no repositories', async () => {
		const gh = fakeGitHub({
			[`${API}/user/installations?per_page=100`]: () =>
				json({
					installations: [
						{ id: 1, account: { login: 'a' }, suspended_at: '2026-01-01T00:00:00Z' },
						{ id: 2, account: { login: 'b' }, suspended_at: null },
						{ id: 3, account: { login: 'c' } }
					]
				}),
			[`${API}/user/installations/2/repositories?per_page=100`]: () =>
				new Response('gone', { status: 404 }),
			[`${API}/user/installations/3/repositories?per_page=100`]: () =>
				json({ repositories: [repo('c/z')] })
		});
		expect((await fetchUserRepositories('t', gh.fn)).map((r) => r.repo)).toEqual(['github:c/z']);
		expect(gh.calls.some((c) => c.url.includes('/installations/1/'))).toBe(false);
	});

	it.each([401, 403, 500])('a %i from one installation is fatal', async (status) => {
		const gh = fakeGitHub({
			[`${API}/user/installations?per_page=100`]: () =>
				json({
					installations: [
						{ id: 2, account: { login: 'b' } },
						{ id: 3, account: { login: 'c' } }
					]
				}),
			[`${API}/user/installations/2/repositories?per_page=100`]: () =>
				json({ repositories: [repo('b/x')] }),
			[`${API}/user/installations/3/repositories?per_page=100`]: () => new Response('x', { status })
		});
		await expect(fetchUserRepositories('t', gh.fn)).rejects.toThrow(String(status));
	});

	it('uses one overall abort signal and does not follow redirects', async () => {
		const seen: (RequestInit | undefined)[] = [];
		const fn = (async (_u: RequestInfo | URL, init?: RequestInit) => {
			seen.push(init);
			return json({ installations: [] });
		}) as unknown as typeof fetch;
		await fetchUserRepositories('t', fn);
		expect(seen[0]?.redirect).toBe('error');
		expect(seen[0]?.signal).toBeInstanceOf(AbortSignal);
	});

	it('times out when GitHub hangs', async () => {
		const fn = ((_u: RequestInfo | URL, init?: RequestInit) =>
			new Promise<Response>((_res, rej) =>
				init?.signal?.addEventListener('abort', () => rej(init.signal?.reason))
			)) as unknown as typeof fetch;
		await expect(fetchUserRepositories('t', fn, 50)).rejects.toThrow();
	});

	it('fetches installations in parallel, at most 4 at a time', async () => {
		let active = 0;
		let peak = 0;
		const fn = (async (input: RequestInfo | URL) => {
			const url = String(input);
			if (url.includes('/repositories')) {
				active++;
				peak = Math.max(peak, active);
				await new Promise((r) => setTimeout(r, 20));
				active--;
				return json({ repositories: [] });
			}
			return json({
				installations: Array.from({ length: 8 }, (_, i) => ({ id: i + 1, account: { login: 'x' } }))
			});
		}) as unknown as typeof fetch;
		await fetchUserRepositories('t', fn);
		expect(peak).toBeGreaterThan(1);
		expect(peak).toBeLessThanOrEqual(4);
	});
});

describe('user repository snapshot in postgres', () => {
	let container: StartedPostgreSqlContainer;
	let sql: postgres.Sql;
	let userId: number;
	let otherId: number;

	beforeAll(async () => {
		container = await new PostgreSqlContainer('postgres:18').start();
		sql = postgres(container.getConnectionUri(), { onnotice: () => {} });
		await migrate(sql);
		userId = await upsertUser(sql, { id: 1, login: 'a', name: null, avatarUrl: null });
		otherId = await upsertUser(sql, { id: 2, login: 'b', name: null, avatarUrl: null });
	}, 120_000);

	afterAll(async () => {
		await sql?.end();
		await container?.stop();
	});

	const ok = (repos: string[]) =>
		fakeGitHub({
			[`${API}/user/installations?per_page=100`]: () =>
				json({ installations: [{ id: 7, account: { login: 'acme' } }] }),
			[`${API}/user/installations/7/repositories?per_page=100`]: () =>
				json({ repositories: repos.map((r) => repo(r)) })
		}).fn;

	it('starts empty', async () => {
		expect(await hasUserRepositories(sql, userId)).toBe(false);
		expect(await listUserRepositories(sql, userId)).toEqual([]);
	});

	it('stores the snapshot and replaces it on the next sync, per user', async () => {
		expect(await syncUserRepositories(sql, userId, 't', ok(['acme/a', 'acme/b']))).toBe(true);
		expect(await syncUserRepositories(sql, otherId, 't', ok(['acme/z']))).toBe(true);
		expect((await listUserRepositories(sql, userId)).map((r) => r.repo)).toEqual([
			'github:acme/a',
			'github:acme/b'
		]);
		expect(await hasUserRepositories(sql, userId)).toBe(true);

		await syncUserRepositories(sql, userId, 't', ok(['acme/b', 'acme/c']));
		expect((await listUserRepositories(sql, userId)).map((r) => r.repo)).toEqual([
			'github:acme/b',
			'github:acme/c'
		]);
		expect((await listUserRepositories(sql, otherId)).map((r) => r.repo)).toEqual([
			'github:acme/z'
		]);
	});

	it('keeps the previous snapshot when GitHub fails, without throwing', async () => {
		const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
		const failing = fakeGitHub({
			[`${API}/user/installations?per_page=100`]: () =>
				json({ installations: [{ id: 7, account: { login: 'acme' } }] }),
			[`${API}/user/installations/7/repositories?per_page=100`]: () =>
				new Response('boom', { status: 502 })
		}).fn;
		expect(await syncUserRepositories(sql, userId, 't', failing)).toBe(false);
		expect((await listUserRepositories(sql, userId)).map((r) => r.repo)).toEqual([
			'github:acme/b',
			'github:acme/c'
		]);
		expect(spy).toHaveBeenCalled();
		spy.mockRestore();
	});

	it('an empty result clears the snapshot', async () => {
		const none = fakeGitHub({
			[`${API}/user/installations?per_page=100`]: () => json({ installations: [] })
		}).fn;
		await syncUserRepositories(sql, otherId, 't', none);
		expect(await hasUserRepositories(sql, otherId)).toBe(false);
	});

	it('reports when the snapshot was fetched (oldest row), null when empty', async () => {
		const before = Date.now();
		await syncUserRepositories(sql, userId, 't', ok(['acme/a', 'acme/b']));
		const snap = await getUserRepositorySnapshot(sql, userId);
		expect(snap.repositories).toHaveLength(2);
		expect(snap.fetchedAt!.getTime()).toBeGreaterThan(before - 5000);
		expect(snap.fetchedAt!.getTime()).toBeLessThanOrEqual(Date.now());
		await sql`UPDATE app.user_repositories SET fetched_at = now() - interval '2 days'
			WHERE user_id = ${userId} AND repo = 'github:acme/a'`;
		const aged = await getUserRepositorySnapshot(sql, userId);
		expect(Date.now() - aged.fetchedAt!.getTime()).toBeGreaterThan(47 * 3600_000);
		expect(await getUserRepositorySnapshot(sql, otherId)).toEqual({
			repositories: [],
			fetchedAt: null
		});
	});

	it('has no column that could hold a token', async () => {
		const cols = await sql`SELECT column_name FROM information_schema.columns
			WHERE table_schema = 'app' AND table_name = 'user_repositories'`;
		expect(cols.map((c) => c.column_name).sort()).toEqual([
			'account_login',
			'default_branch',
			'fetched_at',
			'installation_id',
			'private',
			'repo',
			'user_id'
		]);
	});
});

describe('installUrl', () => {
	it('uses the configured slug', () => {
		expect(installUrl('my-app')).toBe('https://github.com/apps/my-app/installations/new');
	});
});
