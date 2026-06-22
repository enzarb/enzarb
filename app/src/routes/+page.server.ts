import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals }) => {
	if (locals.session?.orgs?.length) {
		redirect(302, `/dashboard`);
	}
};
