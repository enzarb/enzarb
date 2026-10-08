import { redirect } from '@sveltejs/kit';
import { generateState } from 'arctic';
import { githubClient, setOauthStateCookie } from '#lib/server/auth.ts';
import type { RequestHandler } from './$types';

export const GET: RequestHandler = ({ cookies }) => {
	const state = generateState();
	setOauthStateCookie(cookies, state);
	redirect(302, githubClient().createAuthorizationURL(state, ['read:user']).toString());
};
