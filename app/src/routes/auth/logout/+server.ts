import { redirect } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { destroySession } from '$lib/session';

export const POST: RequestHandler = async ({ cookies }) => {
	const sessionId = cookies.get('session');
	if (sessionId) {
		await destroySession(sessionId);
		cookies.delete('session', { path: '/' });
	}
	redirect(302, '/');
};
