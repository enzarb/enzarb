import type { PageServerLoad } from './$types';
import { getRepositories } from '$remote/registry';

export const load: PageServerLoad = async ({ params }) => {
	const repos = await getRepositories(params.org);
	return { repos };
};
