import type { Handle } from '@sveltejs/kit';
import { getSession } from '$lib/session';
import { initKeys } from '$lib/jwt';
import { migrate } from '$lib/db';

let initialized = false;
async function init() {
	if (initialized) return;
	initialized = true;
	await migrate();
	await initKeys();
}

export const handle: Handle = async ({ event, resolve }) => {
	await init();
	event.locals.session = await getSession(event);
	const response = await resolve(event);
	response.headers.set('X-Frame-Options', 'DENY');
	response.headers.set('X-Content-Type-Options', 'nosniff');
	response.headers.set('Referrer-Policy', 'strict-origin-when-cross-origin');
	response.headers.set(
		'Content-Security-Policy',
		"default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'"
	);
	return response;
};
