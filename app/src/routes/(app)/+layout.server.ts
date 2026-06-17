import { redirect } from '@sveltejs/kit';
import type { LayoutServerLoad } from './$types';

export const load: LayoutServerLoad = async ({ locals, url }) => {
	if (!locals.session) {
		redirect(302, `/auth/login?returnTo=${encodeURIComponent(url.pathname)}`);
	}
	return { session: locals.session };
};
