import { isRedirect } from '@sveltejs/kit';
import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import { migrate } from '../../../../../scripts/migrate.ts';
import { OAUTH_STATE_COOKIE } from '#lib/server/auth.ts';
import { OAUTH_RETURN_COOKIE } from '#lib/server/return-path.ts';

let container: StartedPostgreSqlContainer;
let sql: postgres.Sql;
const TOKEN = 'ghu_secret_user_token';

const json = (body: unknown) =>
	new Response(JSON.stringify(body), { headers: { 'content-type': 'application/json' } });

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

async function callback(returnCookie?: string, oldSessionId?: string) {
	const { GET } = await import('./+server.ts');
	const jar = new Map<string, string>([[OAUTH_STATE_COOKIE, 's1']]);
	if (returnCookie) jar.set(OAUTH_RETURN_COOKIE, returnCookie);
	const cookies = {
		get: (n: string) => jar.get(n),
		set: (n: string, v: string) => void jar.set(n, v),
		delete: (n: string) => void jar.delete(n)
	};
	const url = new URL('https://enzarb.example/login/github/callback?code=c&state=s1');
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
});
