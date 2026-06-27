import type { Handle, HandleServerError } from '@sveltejs/kit';
import { getSession } from '$lib/session';
import { initKeys } from '$lib/jwt';
import { migrate, sql } from '$lib/db';

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

export const handleError: HandleServerError = async ({ error, event, status }) => {
	const autoScope = status === 401 || status === 403 ? 'security' : 'application';
	const scope = (error as any)?.scope ?? autoScope;
	const message = error instanceof Error ? error.message : String(error);
	const stack = error instanceof Error ? (error.stack ?? null) : null;
	try {
		await sql`
			INSERT INTO error_logs (scope, level, message, stack, context, user_id, ip_address)
			VALUES (
				${scope}, 'error', ${message}, ${stack},
				${JSON.stringify({ status, path: event.url.pathname })},
				${(event.locals as any).session?.userId ?? null},
				${event.request.headers.get('x-forwarded-for') ?? null}
			)
		`;
	} catch {
		// Never let logging failure cascade to the user
	}
	return { message: status >= 500 ? 'An unexpected error occurred.' : message };
};
