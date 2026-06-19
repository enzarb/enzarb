import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';
import { sql } from '$lib/db';

export const load: PageServerLoad = async ({ locals, url }) => {
	if (!locals.session) {
		const here = encodeURIComponent('/onboarding?' + url.searchParams.toString());
		redirect(302, `/auth/login?returnTo=${here}`);
	}
	const rows = await sql`SELECT username FROM users WHERE id = ${locals.session.userId}`;
	if (rows[0]?.username) redirect(302, '/dashboard');
	return {};
};
