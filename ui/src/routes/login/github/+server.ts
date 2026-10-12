import { error, redirect } from '@sveltejs/kit';
import { generateState } from 'arctic';
import {
	CREATE_REPO_INTENT_COOKIE,
	CREATE_REPO_PURPOSE,
	OAUTH_PURPOSE_COOKIE,
	deleteCreateRepoIntentCookie,
	githubClient,
	setOauthPurposeCookie,
	setOauthReturnCookie,
	setOauthStateCookie
} from '#lib/server/auth.ts';
import { getConfig } from '#lib/server/config.ts';
import { stateWithNonce, verifyIntent } from '#lib/server/create-repo-intent.ts';
import { safeReturnPath } from '#lib/server/return-path.ts';
import type { RequestHandler } from './$types';

/**
 * Starts GitHub sign-in. `?return=/path` (same-origin relative paths only) is where to land
 * afterwards; `?refresh=1` is the same flow, used as the App's Setup URL and by the
 * "Refresh repositories" action: GitHub skips consent when already authorized, and the callback
 * re-reads the user's installations.
 *
 * `?purpose=create-repo` is the re-authorization of `startCreateRepo`: it needs a session and the
 * signed intent cookie, and binds the OAuth state to the intent's nonce.
 */
export const GET: RequestHandler = async ({ cookies, url, locals }) => {
	let state = generateState();
	if (url.searchParams.get('purpose') === CREATE_REPO_PURPOSE) {
		const intent = await verifyIntent(
			getConfig().sessionJwtSecret,
			cookies.get(CREATE_REPO_INTENT_COOKIE)
		);
		if (!locals.user || !intent || intent.userId !== locals.user.id) {
			deleteCreateRepoIntentCookie(cookies);
			error(400, 'No valid create-repository request');
		}
		state = stateWithNonce(state, intent.nonce);
		setOauthPurposeCookie(cookies, CREATE_REPO_PURPOSE);
	} else {
		// A stale marker must never turn an ordinary sign-in into a create-repository one.
		cookies.delete(OAUTH_PURPOSE_COOKIE, { path: '/' });
	}
	setOauthStateCookie(cookies, state);
	setOauthReturnCookie(cookies, safeReturnPath(url.searchParams.get('return')));
	// SvelteKit 3 only allows redirects to another origin when the call opts in; allow GitHub only.
	redirect(302, githubClient().createAuthorizationURL(state, ['read:user']).toString(), {
		external: ['https://github.com']
	});
};
