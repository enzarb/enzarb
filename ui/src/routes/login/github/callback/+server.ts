import { error, redirect } from '@sveltejs/kit';
import {
	createSession,
	fetchGitHubProfile,
	githubClient,
	OAUTH_STATE_COOKIE,
	setSessionCookie,
	upsertUser
} from '#lib/server/auth.ts';
import { getSql } from '#lib/server/db.ts';
import type { RequestHandler } from './$types';

export const GET: RequestHandler = async ({ url, cookies }) => {
	const code = url.searchParams.get('code');
	const state = url.searchParams.get('state');
	const expected = cookies.get(OAUTH_STATE_COOKIE);
	cookies.delete(OAUTH_STATE_COOKIE, { path: '/' });
	if (!code || !state || !expected || state !== expected) error(400, 'Invalid OAuth state');

	let accessToken: string;
	try {
		accessToken = (await githubClient().validateAuthorizationCode(code)).accessToken();
	} catch {
		error(400, 'GitHub rejected the authorization code');
	}

	const sql = getSql();
	const userId = await upsertUser(sql, await fetchGitHubProfile(accessToken));
	const { token, session } = await createSession(sql, userId);
	setSessionCookie(cookies, token, session.expiresAt);
	redirect(303, '/');
};
