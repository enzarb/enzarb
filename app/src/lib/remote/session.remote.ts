import { query } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';

export const getSession = query(async () => {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	return locals.session;
});

// Returns session or null — for public pages that need to know auth state.
export const getOptionalSession = query(async () => {
	const { locals } = getRequestEvent();
	return locals.session ?? null;
});
