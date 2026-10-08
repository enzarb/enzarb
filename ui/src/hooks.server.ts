import { redirect } from '@sveltejs/kit';
import type { Handle } from '@sveltejs/kit/hooks';
import {
	deleteSessionCookie,
	SESSION_COOKIE,
	setSessionCookie,
	validateSessionToken
} from '#lib/server/auth.ts';
import { getSql } from '#lib/server/db.ts';

const isPublic = (path: string) => path === '/login' || path.startsWith('/login/');

export const handle: Handle = async ({ event, resolve }) => {
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

	if (!event.locals.user && !isPublic(event.url.pathname)) {
		// Remote function calls (and other non-navigations) get a 401, pages get the login redirect.
		if (event.url.pathname.startsWith('/_app/remote')) {
			return new Response('Unauthorized', { status: 401 });
		}
		redirect(303, '/login');
	}

	return resolve(event);
};
