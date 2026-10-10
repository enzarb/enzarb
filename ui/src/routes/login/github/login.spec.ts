import { isRedirect } from '@sveltejs/kit';
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import { OAUTH_STATE_COOKIE } from '#lib/server/auth.ts';
import { OAUTH_RETURN_COOKIE } from '#lib/server/return-path.ts';

// Regression test for enzarb-ui#28: SvelteKit 3 rejects redirect() to another origin unless the
// call opts in, so "Sign in with GitHub" returned 500 instead of redirecting to GitHub.
beforeAll(() => {
	vi.stubEnv('DATABASE_URL', 'postgres://x@localhost/x');
	vi.stubEnv('ENZARB_SESSION_JWT_SECRET', 'test-secret');
	vi.stubEnv('GITHUB_OAUTH_CLIENT_ID', 'Iv1.testclient');
	vi.stubEnv('GITHUB_OAUTH_CLIENT_SECRET', 'test-client-secret');
	vi.stubEnv('ENZARB_PUBLIC_URL', 'https://enzarb.example');
});

afterAll(() => {
	vi.unstubAllEnvs();
});

describe('GET /login/github', () => {
	it('redirects to the GitHub authorize URL and sets the state cookie', async () => {
		const { GET } = await import('./+server.ts');
		const set = new Map<string, string>();
		const cookies = { set: (name: string, value: string) => set.set(name, value) };
		let thrown: unknown;
		try {
			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			await (GET as any)({
				cookies,
				url: new URL('https://enzarb.example/login/github?refresh=1&return=%2Fprojects')
			});
		} catch (e) {
			thrown = e;
		}
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
	});

	it('ignores a return target that is not a same-origin path', async () => {
		const { GET } = await import('./+server.ts');
		const set = new Map<string, string>();
		const cookies = { set: (name: string, value: string) => set.set(name, value) };
		const url = new URL('https://enzarb.example/login/github?return=https%3A%2F%2Fevil.example');
		try {
			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			(GET as any)({ cookies, url });
		} catch {
			// the handler always ends in a redirect
		}
		expect(set.get(OAUTH_RETURN_COOKIE)).toBe('/');
	});
});
