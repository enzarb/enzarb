import { redirect } from '@sveltejs/kit';
import { building, dev } from '$app/env';
import type { Handle } from '@sveltejs/kit/hooks';
import {
	deleteSessionCookie,
	SESSION_COOKIE,
	setSessionCookie,
	validateSessionToken
} from '#lib/server/auth.ts';
import { getSql } from '#lib/server/db.ts';
import { startNotifier } from '#lib/server/notifier.ts';
import { PROBE_PATHS, isPublicPath } from '#lib/server/public-paths.ts';

// Web Push sender (a no-op without VAPID keys). Not during the build analysis or in tests.
if (!building && !process.env.VITEST) {
	try {
		startNotifier();
	} catch (e) {
		console.warn(`notifier not started: ${e instanceof Error ? e.message : e}`);
	}
}

export const handle: Handle = async ({ event, resolve }) => {
	// Probes must never depend on the database (session lookup) or the auth redirect.
	if (PROBE_PATHS.has(event.url.pathname)) {
		event.locals.user = null;
		event.locals.session = null;
		return resolve(event);
	}

	event.locals.user = null;
	event.locals.session = null;

	const token = event.cookies.get(SESSION_COOKIE);
	if (token) {
		const result = await validateSessionToken(getSql(), token);
		if (result) {
			event.locals.user = result.user;
			event.locals.session = result.session;
			setSessionCookie(event.cookies, token, result.session.expiresAt);
		} else {
			deleteSessionCookie(event.cookies);
		}
	}

	if (!event.locals.user && !isPublicPath(event.url.pathname, dev)) {
		// Remote function calls (and other non-navigations) get a 401, pages get the login redirect.
		if (event.url.pathname.startsWith('/_app/remote')) {
			return new Response('Unauthorized', { status: 401 });
		}
		redirect(303, '/login');
	}

	return resolve(event);
};
