import { redirect } from '@sveltejs/kit';
import { deleteSessionCookie, invalidateSession } from '#lib/server/auth.ts';
import { getSql } from '#lib/server/db.ts';
import type { RequestHandler } from './$types';

// POST only: SvelteKit's origin check protects it from cross-site form posts.
export const POST: RequestHandler = async ({ locals, cookies }) => {
	if (locals.session) await invalidateSession(getSql(), locals.session.id);
	deleteSessionCookie(cookies);
	redirect(303, '/login');
};
