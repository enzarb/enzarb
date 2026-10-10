import { redirect } from '@sveltejs/kit';
import { generateState } from 'arctic';
import { githubClient, setOauthReturnCookie, setOauthStateCookie } from '#lib/server/auth.ts';
import { safeReturnPath } from '#lib/server/return-path.ts';
import type { RequestHandler } from './$types';

/**
 * Starts GitHub sign-in. `?return=/path` (same-origin relative paths only) is where to land
 * afterwards; `?refresh=1` is the same flow, used as the App's Setup URL and by the
 * "Refresh repositories" action: GitHub skips consent when already authorized, and the callback
 * re-reads the user's installations.
 */
export const GET: RequestHandler = ({ cookies, url }) => {
	const state = generateState();
	setOauthStateCookie(cookies, state);
	setOauthReturnCookie(cookies, safeReturnPath(url.searchParams.get('return')));
	// SvelteKit 3 only allows redirects to another origin when the call opts in; allow GitHub only.
	redirect(302, githubClient().createAuthorizationURL(state, ['read:user']).toString(), {
		external: ['https://github.com']
	});
};
