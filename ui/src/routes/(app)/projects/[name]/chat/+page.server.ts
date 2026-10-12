import { error } from '@sveltejs/kit';
import { chatEnabled } from '#lib/server/flags.ts';
import { getProjectStore } from '#lib/server/kube.ts';
import { projectService } from '#lib/server/project-service.ts';
import type { PageServerLoad } from './$types';

const DNS_LABEL = /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/;

/**
 * The chat route exists only when `ENZARB_UI_CHAT=true`; otherwise it is a plain 404. When on, it is
 * for the signed-in owner of the project only (the hook already redirects anonymous page loads).
 */
export const load: PageServerLoad = async ({ locals, params }) => {
	if (!chatEnabled()) error(404, 'Not found');
	if (!locals.user) error(401, 'Not signed in');
	// Project names are DNS labels; anything else cannot be a project, so do not ask the cluster.
	if (!DNS_LABEL.test(params.name)) error(404, 'Project not found');
	const project = await projectService(getProjectStore(), locals.user.login).get(params.name);
	if (!project) error(404, 'Project not found');
	return {};
};
