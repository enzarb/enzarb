import type { PageServerLoad, Actions } from './$types';
import { error } from '@sveltejs/kit';
import { getProjects, createNewProject } from '$remote/projects';

export const load: PageServerLoad = async ({ params, locals }) => {
	const session = locals.session!;
	const org = session.orgs.find((o) => o.id === params.org);
	if (!org) error(403, 'Forbidden');
	const projects = await getProjects(params.org);
	return { org, projects };
};
