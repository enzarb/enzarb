import { error, redirect } from '@sveltejs/kit';
import {
	CREATE_REPO_INTENT_COOKIE,
	CREATE_REPO_PURPOSE,
	OAUTH_PURPOSE_COOKIE,
	createSession,
	deleteCreateRepoIntentCookie,
	fetchGitHubProfile,
	githubClient,
	invalidateSession,
	OAUTH_STATE_COOKIE,
	setSessionCookie,
	upsertUser
} from '#lib/server/auth.ts';
import { getConfig } from '#lib/server/config.ts';
import { runCreateRepo, type RepoOutcome } from '#lib/server/create-repo-flow.ts';
import {
	intentMatchesProfile,
	stateHasNonce,
	verifyIntent,
	type CreateRepoIntent
} from '#lib/server/create-repo-intent.ts';
import { getSql } from '#lib/server/db.ts';
import { OAUTH_RETURN_COOKIE, safeReturnPath } from '#lib/server/return-path.ts';
import { syncUserRepositories } from '#lib/server/user-repositories.ts';
import type { RequestHandler } from './$types';

/** `path` with the create-repository result added; `repoResult` is always from the fixed enum. */
function withResult(path: string, outcome: Pick<RepoOutcome, 'code' | 'status' | 'key'>): string {
	const u = new URL(safeReturnPath(path), 'http://return.invalid');
	u.searchParams.set('repoResult', outcome.code);
	if (outcome.status) u.searchParams.set('repoStatus', String(outcome.status));
	if (outcome.key) u.searchParams.set('newRepo', outcome.key);
	return u.pathname + u.search + u.hash;
}

export const GET: RequestHandler = async ({ url, cookies, locals }) => {
	const code = url.searchParams.get('code');
	const state = url.searchParams.get('state');
	const expected = cookies.get(OAUTH_STATE_COOKIE);
	cookies.delete(OAUTH_STATE_COOKIE, { path: '/' });
	const returnTo = safeReturnPath(cookies.get(OAUTH_RETURN_COOKIE));
	cookies.delete(OAUTH_RETURN_COOKIE, { path: '/' });
	const createRepo = cookies.get(OAUTH_PURPOSE_COOKIE) === CREATE_REPO_PURPOSE;
	cookies.delete(OAUTH_PURPOSE_COOKIE, { path: '/' });
	// The intent is single-use: read it, then delete it before anything else can fail.
	const intentCookie = cookies.get(CREATE_REPO_INTENT_COOKIE);
	deleteCreateRepoIntentCookie(cookies);
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

	let intent: CreateRepoIntent | null = null;
	if (createRepo) {
		intent = await verifyIntent(getConfig().sessionJwtSecret, intentCookie);
		if (!intent || !stateHasNonce(state, intent.nonce))
			error(400, 'Invalid create-repository request');
	}

	const sql = getSql();
	const profile = await fetchGitHubProfile(accessToken);
	// The browser may have authorized as another GitHub account than the one that asked for the
	// repository. Refuse before any database write: no user row, session or snapshot for them,
	// and the current session stays.
	if (intent && !intentMatchesProfile(intent, profile))
		redirect(303, withResult(intent.ret, { code: 'identity-mismatch' }));

	const userId = await upsertUser(sql, profile);
	let outcome: RepoOutcome | null = null;
	if (intent) {
		// Creates the repository, then re-syncs the snapshot with the same token.
		outcome = await runCreateRepo(sql, userId, accessToken, intent);
	} else {
		// The token is used for the snapshot only and then dropped; failures never block sign-in.
		await syncUserRepositories(sql, userId, accessToken);
	}
	const { token, session } = await createSession(sql, userId);
	setSessionCookie(cookies, token, session.expiresAt);
	// Re-authenticating (e.g. repository refresh): drop the session this replaces.
	if (locals.session && locals.session.id !== session.id)
		await invalidateSession(sql, locals.session.id);
	redirect(303, intent && outcome ? withResult(intent.ret, outcome) : returnTo);
};
