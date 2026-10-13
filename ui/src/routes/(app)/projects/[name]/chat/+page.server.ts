import { error } from '@sveltejs/kit';
import { getProjectStore } from '#lib/server/kube.ts';
import { projectService } from '#lib/server/project-service.ts';
import type { PageServerLoad } from './$types';

const DNS_LABEL = /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/;

/**
 * The chat route is for the signed-in owner of the project only (the hook already redirects
 * anonymous page loads). Whether the sandbox supports chat is decided on the connection (`acp`).
 */
export const load: PageServerLoad = async ({ locals, params }) => {
	if (!locals.user) error(401, 'Not signed in');
	// Project names are DNS labels; anything else cannot be a project, so do not ask the cluster.
	if (!DNS_LABEL.test(params.name)) error(404, 'Project not found');
	const project = await projectService(getProjectStore(), locals.user.login).get(params.name);
	if (!project) error(404, 'Project not found');
	return {};
};
