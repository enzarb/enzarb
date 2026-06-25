import type { Handle } from '@sveltejs/kit';
import { getSession } from '$lib/session';
import { initKeys } from '$lib/jwt';
import { migrate } from '$lib/db';

let initPromise: Promise<void> | null = null;
async function init() {
	if (!initPromise) {
		initPromise = migrate()
			.then(() => initKeys())
			.catch((err) => {
				// Allow retry on next request if initialization fails.
				initPromise = null;
				throw err;
			});
	}
	await initPromise;
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
