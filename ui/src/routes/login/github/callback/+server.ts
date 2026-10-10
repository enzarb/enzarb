import { error, redirect } from '@sveltejs/kit';
import {
	createSession,
	fetchGitHubProfile,
	githubClient,
	invalidateSession,
	OAUTH_STATE_COOKIE,
	setSessionCookie,
	upsertUser
} from '#lib/server/auth.ts';
import { getSql } from '#lib/server/db.ts';
import { OAUTH_RETURN_COOKIE, safeReturnPath } from '#lib/server/return-path.ts';
import { syncUserRepositories } from '#lib/server/user-repositories.ts';
import type { RequestHandler } from './$types';

export const GET: RequestHandler = async ({ url, cookies, locals }) => {
	const code = url.searchParams.get('code');
	const state = url.searchParams.get('state');
	const expected = cookies.get(OAUTH_STATE_COOKIE);
	cookies.delete(OAUTH_STATE_COOKIE, { path: '/' });
	const returnTo = safeReturnPath(cookies.get(OAUTH_RETURN_COOKIE));
	cookies.delete(OAUTH_RETURN_COOKIE, { path: '/' });
	// GitHub sends the user here without `state` after an App install when "Request user
	// authorization during installation" is on. That code has no CSRF protection, so it is never
	// exchanged: start a normal state-protected sign-in instead (it also refreshes the snapshot).
	if (!state && (url.searchParams.has('setup_action') || url.searchParams.has('installation_id')))
		redirect(303, '/login/github?refresh=1');
	if (!code || !state || !expected || state !== expected) error(400, 'Invalid OAuth state');

	let accessToken: string;
	try {
		accessToken = (await githubClient().validateAuthorizationCode(code)).accessToken();
	} catch {
		error(400, 'GitHub rejected the authorization code');
	}

	const sql = getSql();
	const userId = await upsertUser(sql, await fetchGitHubProfile(accessToken));
	// The token is used for the snapshot only and then dropped; failures never block sign-in.
	await syncUserRepositories(sql, userId, accessToken);
	const { token, session } = await createSession(sql, userId);
	setSessionCookie(cookies, token, session.expiresAt);
	// Re-authenticating (e.g. repository refresh): drop the session this replaces.
	if (locals.session && locals.session.id !== session.id)
		await invalidateSession(sql, locals.session.id);
	redirect(303, returnTo);
};
