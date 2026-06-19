import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

// Redirect single-org users directly to their projects list.
export const load: PageServerLoad = async ({ locals }) => {
	const session = locals.session!;
	if (session.orgs.length === 1) {
		redirect(302, `/orgs/${session.orgs[0].id}/projects`);
	}
	return {};
};
