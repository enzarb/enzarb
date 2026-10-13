import { isRedirect } from '@sveltejs/kit';
import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { migrate } from '../../../../../scripts/migrate.ts';
import { OAUTH_STATE_COOKIE } from '#lib/server/auth.ts';
import { signIntent, type CreateRepoIntent } from '#lib/server/create-repo-intent.ts';
import { OAUTH_RETURN_COOKIE } from '#lib/server/return-path.ts';

let container: StartedPostgreSqlContainer;
let sql: postgres.Sql;
const TOKEN = 'ghu_secret_user_token';

const json = (body: unknown, status = 200) =>
	new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});

function stubGitHub(opts: { reposStatus?: number }) {
	vi.stubGlobal(
		'fetch',
		vi.fn(async (input: RequestInfo | URL) => {
			const url = input instanceof Request ? input.url : String(input);
			if (url.includes('/login/oauth/access_token'))
				return json({ access_token: TOKEN, token_type: 'bearer' });
			if (url === 'https://api.github.com/user')
				return json({ id: 99, login: 'octo', name: 'Octo', avatar_url: null });
			if (url.startsWith('https://api.github.com/user/installations?'))
				return json({ installations: [{ id: 5, account: { login: 'Octo' } }] });
			if (url.startsWith('https://api.github.com/user/installations/5/repositories'))
				return opts.reposStatus
					? new Response('x', { status: opts.reposStatus })
					: json({
							repositories: [{ full_name: 'Octo/Repo', private: true, default_branch: 'main' }]
						});
			return new Response('unexpected ' + url, { status: 500 });
		})
	);
}

async function callback(returnCookie?: string, oldSessionId?: string, query = 'code=c&state=s1') {
	const { GET } = await import('./+server.ts');
	const jar = new Map<string, string>([[OAUTH_STATE_COOKIE, 's1']]);
	if (returnCookie) jar.set(OAUTH_RETURN_COOKIE, returnCookie);
	const cookies = {
		get: (n: string) => jar.get(n),
		set: (n: string, v: string) => void jar.set(n, v),
		delete: (n: string, o?: { path?: string }) => {
			jar.delete(n);
			jar.set('deleted:' + n, o?.path ?? '');
		}
	};
	const url = new URL('https://enzarb.example/login/github/callback?' + query);
	let thrown: unknown;
	try {
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		await (GET as any)({
			url,
			cookies,
			locals: { session: oldSessionId ? { id: oldSessionId } : null }
		});
	} catch (e) {
		thrown = e;
	}
	return { thrown, jar };
}

beforeAll(async () => {
	container = await new PostgreSqlContainer('postgres:18').start();
	sql = postgres(container.getConnectionUri(), { onnotice: () => {} });
	await migrate(sql);
	vi.stubEnv('DATABASE_URL', container.getConnectionUri());
	vi.stubEnv('ENZARB_SESSION_JWT_SECRET', 'test-secret');
	vi.stubEnv('ENZARB_REALTIME_JWT_SECRET', 'test-realtime-secret');
	vi.stubEnv('GITHUB_OAUTH_CLIENT_ID', 'Iv1.testclient');
	vi.stubEnv('GITHUB_OAUTH_CLIENT_SECRET', 'test-client-secret');
	vi.stubEnv('ENZARB_PUBLIC_URL', 'https://enzarb.example');
}, 120_000);

afterAll(async () => {
	vi.unstubAllEnvs();
	vi.unstubAllGlobals();
	const { getSql } = await import('#lib/server/db.ts');
	await getSql().end();
	await sql?.end();
	await container?.stop();
});

describe('GET /login/github/callback', () => {
	it('stores the repository snapshot, never the token, and returns to the requested page', async () => {
		const logs = vi.spyOn(console, 'error').mockImplementation(() => {});
		stubGitHub({});
		const { thrown, jar } = await callback('/projects');
		expect(isRedirect(thrown)).toBe(true);
		expect(thrown).toMatchObject({ status: 303, location: '/projects' });
		expect(jar.has(OAUTH_RETURN_COOKIE)).toBe(false);
		// a previous user's realtime stream cookie is cleared at login (same Path it was set with)
		expect(jar.get('deleted:enzarb_live')).toBe('/live/');
		const rows = await sql`SELECT repo, account_login, private FROM app.user_repositories`;
		expect(rows).toEqual([{ repo: 'github:octo/repo', account_login: 'Octo', private: true }]);
		// The token appears nowhere in the database.
		const dump = JSON.stringify({
			users: await sql`SELECT * FROM app.users`,
			sessions: await sql`SELECT * FROM app.sessions`,
			repos: await sql`SELECT * FROM app.user_repositories`
		});
		expect(dump).not.toContain(TOKEN);
		expect(JSON.stringify([...jar])).not.toContain(TOKEN);
		expect(JSON.stringify(logs.mock.calls)).not.toContain(TOKEN);
		logs.mockRestore();
	});

	it('does not block sign-in when the GitHub API fails, and keeps the old snapshot', async () => {
		const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
		stubGitHub({ reposStatus: 500 });
		const { thrown } = await callback('https://evil.example');
		expect(thrown).toMatchObject({ status: 303, location: '/' });
		expect((await sql`SELECT repo FROM app.user_repositories`).map((r) => r.repo)).toEqual([
			'github:octo/repo'
		]);
		expect((await sql`SELECT count(*)::int AS n FROM app.sessions`)[0].n).toBe(2);
		expect(JSON.stringify(spy.mock.calls)).not.toContain(TOKEN);
		spy.mockRestore();
	});

	it('invalidates the session it replaces when re-authenticating', async () => {
		stubGitHub({});
		const [old] = await sql`SELECT id FROM app.sessions ORDER BY created_at LIMIT 1`;
		const before = (await sql`SELECT count(*)::int AS n FROM app.sessions`)[0].n;
		await callback('/', old.id);
		const ids = (await sql`SELECT id FROM app.sessions`).map((r) => r.id);
		expect(ids).not.toContain(old.id);
		expect(ids).toHaveLength(before);
	});

	describe('purpose=create-repo', () => {
		const SECRET = 'test-secret';
		const STATE = 's1.nonce1';
		let userId: number;

		interface Opts {
			profile?: { id: number; login: string };
			post?: number;
			selection?: 'all' | 'selected';
			put?: number;
			appear?: boolean;
		}
		function stubCreate(o: Opts = {}) {
			const calls: { method: string; url: string; auth: string | null; intentGone?: boolean }[] =
				[];
			let created = false;
			let added = false;
			const selection = o.selection ?? 'all';
			vi.stubGlobal(
				'fetch',
				vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
					const url = input instanceof Request ? input.url : String(input);
					const method = input instanceof Request ? input.method : (init?.method ?? 'GET');
					const auth = new Headers(
						input instanceof Request ? input.headers : (init?.headers as HeadersInit)
					).get('authorization');
					calls.push({
						method,
						url,
						auth,
						intentGone: currentJar?.has('enzarb_create_repo') === false
					});
					if (url.includes('/login/oauth/access_token'))
						return json({ access_token: TOKEN, token_type: 'bearer' });
					if (url === 'https://api.github.com/user')
						return json({
							...(o.profile ?? { id: 99, login: 'octo' }),
							name: null,
							avatar_url: null
						});
					if (method === 'POST' && url.endsWith('/repos')) {
						if ((o.post ?? 201) !== 201)
							return json({ message: 'Resource not accessible' }, o.post);
						created = true;
						return json(
							{
								id: 77,
								full_name: 'Octo/fresh',
								html_url: 'x',
								private: true,
								default_branch: 'main'
							},
							201
						);
					}
					if (method === 'PUT') {
						if (o.put && o.put !== 204) return json({}, o.put);
						added = true;
						return new Response(null, { status: 204 });
					}
					if (url.startsWith('https://api.github.com/user/installations?'))
						return json({
							installations: [
								{ id: 5, account: { login: 'Octo' }, repository_selection: selection }
							]
						});
					if (url.startsWith('https://api.github.com/user/installations/5/repositories')) {
						const visible = created && o.appear !== false && (selection === 'all' || added);
						return json({
							repositories: [
								{ full_name: 'Octo/Repo', private: true, default_branch: 'main' },
								...(visible
									? [{ full_name: 'Octo/fresh', private: true, default_branch: 'main' }]
									: [])
							]
						});
					}
					return new Response('unexpected ' + url, { status: 500 });
				})
			);
			const writes = () =>
				calls.filter((c) => c.method !== 'GET' && !c.url.includes('/login/oauth'));
			return { calls, writes };
		}

		let currentJar: Map<string, string> | undefined;
		async function run(
			intentOver: Partial<CreateRepoIntent> = {},
			cookieOver: Record<string, string | undefined> = {},
			query = `code=c&state=${STATE}`,
			oldSessionId?: string
		) {
			const { GET } = await import('./+server.ts');
			const intent: CreateRepoIntent = {
				owner: 'Octo',
				name: 'fresh',
				visibility: 'private',
				description: 'd',
				userId,
				githubId: 99,
				login: 'octo',
				nonce: 'nonce1',
				ret: '/projects/new',
				...intentOver
			};
			const jar = new Map<string, string>([
				[OAUTH_STATE_COOKIE, STATE],
				['enzarb_oauth_purpose', 'create-repo'],
				[OAUTH_RETURN_COOKIE, '/projects/new'],
				['enzarb_create_repo', await signIntent(SECRET, intent)]
			]);
			for (const [k, v] of Object.entries(cookieOver)) {
				if (v === undefined) jar.delete(k);
				else jar.set(k, v);
			}
			currentJar = jar;
			const sets: string[] = [];
			const cookies = {
				get: (n: string) => jar.get(n),
				set: (n: string, v: string) => (jar.set(n, v), void sets.push(n + '=' + v)),
				delete: (n: string) => void jar.delete(n)
			};
			let thrown: unknown;
			try {
				// eslint-disable-next-line @typescript-eslint/no-explicit-any
				await (GET as any)({
					url: new URL('https://enzarb.example/login/github/callback?' + query),
					cookies,
					locals: { session: oldSessionId ? { id: oldSessionId } : null }
				});
			} catch (e) {
				thrown = e;
			}
			const location = isRedirect(thrown)
				? new URL((thrown as { location: string }).location, 'https://x')
				: null;
			return { thrown, jar, sets, location };
		}
		const counts = async () => ({
			users: (await sql`SELECT count(*)::int AS n FROM app.users`)[0].n,
			sessions: (await sql`SELECT count(*)::int AS n FROM app.sessions`)[0].n
		});

		beforeAll(async () => {
			const { pollDefaults } = await import('#lib/server/create-repo-flow.ts');
			pollDefaults.tries = 2;
			pollDefaults.delayMs = 1;
			const [row] = await sql`INSERT INTO app.users (github_id, login) VALUES (99, 'octo')
				ON CONFLICT (github_id) DO UPDATE SET login = EXCLUDED.login RETURNING id`;
			userId = Number(row.id);
		});
		beforeEach(async () => {
			await sql`DELETE FROM app.user_repositories WHERE user_id = ${userId}`;
			await sql`INSERT INTO app.user_repositories (user_id, repo, installation_id, account_login, private, default_branch)
				VALUES (${userId}, 'github:octo/repo', 5, 'Octo', true, 'main')`;
			vi.spyOn(console, 'log').mockImplementation(() => {});
			vi.spyOn(console, 'error').mockImplementation(() => {});
		});
		afterEach(() => vi.restoreAllMocks());

		it('creates the repository, preselects it, signs in, and never stores the token', async () => {
			const gh = stubCreate();
			const [old] = await sql`SELECT id FROM app.sessions ORDER BY created_at LIMIT 1`;
			const { location, jar, sets } = await run({}, {}, undefined, old.id);
			expect(location?.pathname).toBe('/projects/new');
			expect(location?.searchParams.get('repoResult')).toBe('created');
			expect(location?.searchParams.get('newRepo')).toBe('github:octo/fresh');
			const post = gh.writes();
			expect(post).toHaveLength(1);
			expect(post[0]).toMatchObject({ method: 'POST', url: 'https://api.github.com/user/repos' });
			expect(post[0].auth).toBe(`Bearer ${TOKEN}`);
			// The intent was consumed before the first GitHub write.
			expect(post[0].intentGone).toBe(true);
			expect(jar.has('enzarb_create_repo')).toBe(false);
			expect(sets.some((s) => s.startsWith('enzarb_session='))).toBe(true);
			expect(await sql`SELECT id FROM app.sessions WHERE id = ${old.id}`).toHaveLength(0);
			expect(
				(await sql`SELECT repo FROM app.user_repositories WHERE user_id = ${userId}`).map(
					(r) => r.repo
				)
			).toContain('github:octo/fresh');
			const dump = JSON.stringify({
				users: await sql`SELECT * FROM app.users`,
				sessions: await sql`SELECT * FROM app.sessions`,
				repos: await sql`SELECT * FROM app.user_repositories`,
				sets,
				logs: [vi.mocked(console.log).mock.calls, vi.mocked(console.error).mock.calls]
			});
			expect(dump).not.toContain(TOKEN);
			expect(vi.mocked(console.log).mock.calls.map((c) => c.join(' '))).toContain(
				'create-repo github:octo/fresh created'
			);
		});

		it('adds the repository to a "selected" installation with the same token', async () => {
			const gh = stubCreate({ selection: 'selected' });
			const { location } = await run();
			expect(location?.searchParams.get('repoResult')).toBe('created');
			const [, put] = gh.writes();
			expect(put).toMatchObject({
				method: 'PUT',
				url: 'https://api.github.com/user/installations/5/repositories/77',
				auth: `Bearer ${TOKEN}`
			});
		});

		it('reports created-not-installed when GitHub refuses the add', async () => {
			stubCreate({ selection: 'selected', put: 403 });
			const { location } = await run();
			expect(location?.searchParams.get('repoResult')).toBe('created-not-installed');
			expect(location?.searchParams.get('repoStatus')).toBe('403');
			expect(location?.searchParams.get('newRepo')).toBe('github:octo/fresh');
		});

		it('reports GitHub 403 with its status, stops, and still signs in', async () => {
			const gh = stubCreate({ post: 403 });
			const { location, sets } = await run();
			expect(location?.searchParams.get('repoResult')).toBe('forbidden');
			expect(location?.searchParams.get('repoStatus')).toBe('403');
			expect(location?.searchParams.has('newRepo')).toBe(false);
			expect(gh.writes()).toHaveLength(1); // no retry, no other token
			// The code exchange authenticates with the OAuth client credentials (not a token); every
			// other GitHub call must carry only the user-to-server token.
			const apiCalls = gh.calls.filter((c) => !c.url.includes('/login/oauth/'));
			expect(new Set(apiCalls.map((c) => c.auth).filter(Boolean))).toEqual(
				new Set([`Bearer ${TOKEN}`])
			);
			expect(sets.some((s) => s.startsWith('enzarb_session='))).toBe(true);
		});

		it('gives the timeout result when the repository does not reach the snapshot', async () => {
			stubCreate({ appear: false });
			const { location, jar } = await run();
			expect(location?.searchParams.get('repoResult')).toBe('timeout');
			expect(jar.has('enzarb_create_repo')).toBe(false);
		});

		it('refuses a different GitHub account before any write', async () => {
			const gh = stubCreate({ profile: { id: 100, login: 'mallory' } });
			const [old] = await sql`SELECT id FROM app.sessions ORDER BY created_at LIMIT 1`;
			const before = await counts();
			const snapshotBefore = JSON.stringify(
				await sql`SELECT * FROM app.user_repositories ORDER BY user_id, repo`
			);
			const { location, jar, sets } = await run({}, {}, undefined, old.id);
			expect(location?.searchParams.get('repoResult')).toBe('identity-mismatch');
			expect(location?.pathname).toBe('/projects/new');
			expect(gh.writes()).toHaveLength(0);
			expect(await counts()).toEqual(before); // no user row, no session
			expect(await sql`SELECT 1 FROM app.users WHERE github_id = 100`).toHaveLength(0);
			expect(
				JSON.stringify(await sql`SELECT * FROM app.user_repositories ORDER BY user_id, repo`)
			).toBe(snapshotBefore);
			expect(gh.calls.some((c) => c.url.includes('/user/installations'))).toBe(false); // no sync
			expect(sets.some((s) => s.startsWith('enzarb_session='))).toBe(false);
			expect(await sql`SELECT 1 FROM app.sessions WHERE id = ${old.id}`).toHaveLength(1);
			expect(jar.has('enzarb_create_repo')).toBe(false);
		});

		it('matches by GitHub id (login casing may differ) but not by login alone', async () => {
			stubCreate({ profile: { id: 99, login: 'OCTO' } });
			expect((await run()).location?.searchParams.get('repoResult')).toBe('created');
			const gh = stubCreate({ profile: { id: 5000, login: 'octo' } });
			const { location } = await run();
			expect(location?.searchParams.get('repoResult')).toBe('identity-mismatch');
			expect(gh.writes()).toHaveLength(0);
		});

		it('rejects a bad state, a bad signature, an expired intent and a nonce mismatch, deleting the intent', async () => {
			const cases: [Partial<CreateRepoIntent>, Record<string, string | undefined>, string][] = [
				[{}, {}, 'code=c&state=wrong'],
				[{}, { enzarb_create_repo: 'tampered.jwt.value' }, `code=c&state=${STATE}`],
				[{}, { enzarb_create_repo: undefined }, `code=c&state=${STATE}`],
				[{ nonce: 'other' }, {}, `code=c&state=${STATE}`]
			];
			for (const [intent, cookieOver, query] of cases) {
				const gh = stubCreate();
				const before = await counts();
				const { thrown, jar } = await run(intent, cookieOver, query);
				expect(thrown).toMatchObject({ status: 400 });
				expect(jar.has('enzarb_create_repo')).toBe(false);
				expect(gh.writes()).toHaveLength(0);
				expect(await counts()).toEqual(before);
			}
		});

		it('refuses an expired intent', async () => {
			const stale = await signIntent(
				SECRET,
				{
					owner: 'Octo',
					name: 'fresh',
					visibility: 'private',
					description: '',
					userId,
					githubId: 99,
					login: 'octo',
					nonce: 'nonce1',
					ret: '/projects/new'
				},
				Math.floor(Date.now() / 1000) - 3600
			);
			const gh = stubCreate();
			const { thrown, jar } = await run({}, { enzarb_create_repo: stale });
			expect(thrown).toMatchObject({ status: 400 });
			expect(jar.has('enzarb_create_repo')).toBe(false);
			expect(gh.writes()).toHaveLength(0);
		});

		it('replaying the flow without the consumed intent creates nothing', async () => {
			const first = stubCreate();
			await run();
			expect(first.writes()).toHaveLength(1);
			const replay = stubCreate();
			const { thrown } = await run({}, { enzarb_create_repo: undefined });
			expect(thrown).toMatchObject({ status: 400 });
			expect(replay.writes()).toHaveLength(0);
		});

		it('refuses an owner that is not among the user installations', async () => {
			const gh = stubCreate();
			const { location } = await run({ owner: 'evil-org' });
			expect(location?.searchParams.get('repoResult')).toBe('owner-not-allowed');
			expect(gh.writes()).toHaveLength(0);
		});

		it('sends the return path through safeReturnPath', async () => {
			stubCreate();
			const { location } = await run({ ret: 'https://evil.example' });
			expect(location?.pathname).toBe('/');
			expect(location?.hostname).toBe('x');
		});

		it('leaves the ordinary sign-in untouched: the purpose-less callback ignores a stray intent', async () => {
			const gh = stubCreate();
			const { location } = await run({}, { enzarb_oauth_purpose: undefined });
			expect(location?.pathname).toBe('/projects/new');
			expect(location?.search).toBe('');
			expect(gh.writes()).toHaveLength(0);
		});
	});

	describe('install-time callback', () => {
		it('without state redirects to the refresh flow; no token exchange, no session', async () => {
			stubGitHub({});
			const fetchMock = vi.mocked(fetch);
			fetchMock.mockClear();
			const before = (await sql`SELECT count(*)::int AS n FROM app.sessions`)[0].n;
			const { thrown, jar } = await callback(
				undefined,
				undefined,
				'code=c&installation_id=5&setup_action=install'
			);
			expect(isRedirect(thrown)).toBe(true);
			expect(thrown).toMatchObject({ status: 303, location: '/login/github?refresh=1' });
			expect(fetchMock).not.toHaveBeenCalled();
			expect((await sql`SELECT count(*)::int AS n FROM app.sessions`)[0].n).toBe(before);
			expect(jar.has('enzarb_session')).toBe(false);
		});

		it('still 400s a missing state without install params', async () => {
			const { thrown } = await callback(undefined, undefined, 'code=c');
			expect(thrown).toMatchObject({ status: 400 });
		});

		it('still 400s a wrong state even with install params', async () => {
			const { thrown } = await callback(
				undefined,
				undefined,
				'code=c&state=wrong&installation_id=5&setup_action=install'
			);
			expect(thrown).toMatchObject({ status: 400 });
		});
	});
});
