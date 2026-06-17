import type { PageServerLoad } from './$types';
import { redirect } from '@sveltejs/kit';

export const load: PageServerLoad = async ({ locals }) => {
	const session = locals.session!;
	// Redirect to first org if available
	if (session.orgs.length === 1) {
		redirect(302, `/orgs/${session.orgs[0].id}/projects`);
	}
	return { orgs: session.orgs };
};
