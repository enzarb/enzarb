import type { LayoutServerLoad } from './$types';
import { error } from '@sveltejs/kit';
import { getProject } from '$lib/k8s';

export const load: LayoutServerLoad = async ({ params, locals }) => {
	const session = locals.session!;
	const org = session.orgs.find((o) => o.id === params.org);
	if (!org) error(403, 'Forbidden');
	const project = await getProject(params.org, params.project) as any;
	if (!project) error(404, 'Project not found');
	return { org, project };
};
