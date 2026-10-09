import { redirect } from '@sveltejs/kit';
import { generateState } from 'arctic';
import { githubClient, setOauthStateCookie } from '#lib/server/auth.ts';
import type { RequestHandler } from './$types';

export const GET: RequestHandler = ({ cookies }) => {
	const state = generateState();
	setOauthStateCookie(cookies, state);
	// SvelteKit 3 only allows redirects to another origin when the call opts in; allow GitHub only.
	redirect(302, githubClient().createAuthorizationURL(state, ['read:user']).toString(), {
		external: ['https://github.com']
	});
};
