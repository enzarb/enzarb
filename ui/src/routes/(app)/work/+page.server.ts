import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

/** `/work` is the old path of `/tasks`; older notifications still link to it. */
export const load: PageServerLoad = ({ url }) => {
	redirect(308, `/tasks${url.search}`);
};
