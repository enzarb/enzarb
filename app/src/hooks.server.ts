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
	// Content-Security-Policy is emitted by SvelteKit (configured in vite.config.ts
	// kit.csp) so its own inline hydration scripts receive a per-request nonce.
	return response;
};
