import { isRedirect } from '@sveltejs/kit';
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import {
	CREATE_REPO_INTENT_COOKIE,
	OAUTH_PURPOSE_COOKIE,
	OAUTH_STATE_COOKIE
} from '#lib/server/auth.ts';
import { signIntent, type CreateRepoIntent } from '#lib/server/create-repo-intent.ts';
import { OAUTH_RETURN_COOKIE } from '#lib/server/return-path.ts';

// Regression test for enzarb-ui#28: SvelteKit 3 rejects redirect() to another origin unless the
// call opts in, so "Sign in with GitHub" returned 500 instead of redirecting to GitHub.
beforeAll(() => {
	vi.stubEnv('DATABASE_URL', 'postgres://x@localhost/x');
	vi.stubEnv('ENZARB_SESSION_JWT_SECRET', 'test-secret');
	vi.stubEnv('ENZARB_REALTIME_JWT_SECRET', 'test-realtime-secret');
	vi.stubEnv('GITHUB_OAUTH_CLIENT_ID', 'Iv1.testclient');
	vi.stubEnv('GITHUB_OAUTH_CLIENT_SECRET', 'test-client-secret');
	vi.stubEnv('ENZARB_PUBLIC_URL', 'https://enzarb.example');
});

afterAll(() => {
	vi.unstubAllEnvs();
});

function jar(initial: Record<string, string> = {}) {
	const set = new Map<string, string>(Object.entries(initial));
	const deleted: string[] = [];
	const cookies = {
		get: (name: string) => set.get(name),
		set: (name: string, value: string) => set.set(name, value),
		delete: (name: string) => {
			set.delete(name);
			deleted.push(name);
		}
	};
	return { set, deleted, cookies };
}

async function run(
	cookies: unknown,
	url: string,
	locals: { user: { id: number } | null } = { user: null }
) {
	const { GET } = await import('./+server.ts');
	try {
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		await (GET as any)({ cookies, url: new URL(url), locals });
	} catch (e) {
		return e;
	}
	return undefined;
}

describe('GET /login/github', () => {
	it('redirects to the GitHub authorize URL and sets the state cookie', async () => {
		const { set, cookies } = jar();
		const thrown = await run(
			cookies,
			'https://enzarb.example/login/github?refresh=1&return=%2Fprojects'
		);
		expect(isRedirect(thrown), 'the handler must throw a SvelteKit redirect').toBe(true);
		expect(thrown).toMatchObject({ status: 302 });
		const location = new URL((thrown as { location: string }).location);
		expect(location.origin + location.pathname).toBe('https://github.com/login/oauth/authorize');
		expect(location.searchParams.get('client_id')).toBe('Iv1.testclient');
		expect(location.searchParams.get('redirect_uri')).toBe(
			'https://enzarb.example/login/github/callback'
		);
		expect(location.searchParams.get('state')).toBeTruthy();
		expect(set.get(OAUTH_STATE_COOKIE)).toBe(location.searchParams.get('state'));
		expect(set.get(OAUTH_RETURN_COOKIE)).toBe('/projects');
		expect(set.has(OAUTH_PURPOSE_COOKIE)).toBe(false);
	});

	it('ignores a return target that is not a same-origin path', async () => {
		const { set, cookies } = jar();
		await run(cookies, 'https://enzarb.example/login/github?return=https%3A%2F%2Fevil.example');
		expect(set.get(OAUTH_RETURN_COOKIE)).toBe('/');
	});

	it('clears a stale purpose marker on an ordinary sign-in', async () => {
		const { set, cookies } = jar({ [OAUTH_PURPOSE_COOKIE]: 'create-repo' });
		await run(cookies, 'https://enzarb.example/login/github');
		expect(set.has(OAUTH_PURPOSE_COOKIE)).toBe(false);
	});

	describe('purpose=create-repo', () => {
		const intent: CreateRepoIntent = {
			owner: 'acme',
			name: 'new',
			visibility: 'private',
			description: '',
			userId: 1,
			githubId: 99,
			login: 'octo',
			nonce: 'nonce1',
			ret: '/projects/new'
		};
		const url = 'https://enzarb.example/login/github?purpose=create-repo&return=%2Fprojects%2Fnew';

		it('binds the state to the intent nonce and marks the purpose', async () => {
			const { set, cookies } = jar({
				[CREATE_REPO_INTENT_COOKIE]: await signIntent('test-secret', intent)
			});
			const thrown = await run(cookies, url, { user: { id: 1 } });
			expect(thrown).toMatchObject({ status: 302 });
			expect(set.get(OAUTH_STATE_COOKIE)).toMatch(/\.nonce1$/);
			expect(set.get(OAUTH_PURPOSE_COOKIE)).toBe('create-repo');
			expect(set.get(OAUTH_RETURN_COOKIE)).toBe('/projects/new');
		});

		it('refuses without a session, with a bad intent or with another user, deleting the intent', async () => {
			const good = await signIntent('test-secret', intent);
			for (const [token, locals] of [
				[good, { user: null }],
				['garbage', { user: { id: 1 } }],
				[undefined, { user: { id: 1 } }],
				[good, { user: { id: 2 } }]
			] as const) {
				const { set, deleted, cookies } = jar(token ? { [CREATE_REPO_INTENT_COOKIE]: token } : {});
				const thrown = await run(cookies, url, locals);
				expect(thrown).toMatchObject({ status: 400 });
				expect(deleted).toContain(CREATE_REPO_INTENT_COOKIE);
				expect(set.has(OAUTH_STATE_COOKIE)).toBe(false);
			}
		});
	});
});
