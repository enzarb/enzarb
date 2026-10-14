import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

/** `/work/[id]` is the old path of `/task/[id]`; links in PR comments and notifications still use it. */
export const load: PageServerLoad = ({ params, url }) => {
	redirect(308, `/task/${encodeURIComponent(params.id)}${url.search}`);
};
