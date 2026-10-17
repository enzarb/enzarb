import { redirect } from '@sveltejs/kit';
import { getConfig } from '#lib/server/config.ts';
import { getProjectStore } from '#lib/server/kube.ts';
import { projectService } from '#lib/server/project-service.ts';
import { getRepositoryAccess } from '#lib/server/repo-access.ts';
import { groupByAccount, installUrl } from '#lib/server/user-repositories.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals }) => {
	if (!locals.user) redirect(303, '/login');
	const [list, projects] = await Promise.all([
		// Live list (display cache); a GitHub failure shows no installations rather than an error.
		getRepositoryAccess()
			.list(locals.user.login)
			.catch(() => null),
		// Claude setup runs in one of the user's own sandboxes; a cluster failure only hides the picker.
		projectService(getProjectStore(), locals.user.login)
			.list()
			.catch(() => [])
	]);
	return {
		account: { login: locals.user.login, name: locals.user.name },
		installUrl: installUrl(getConfig().githubAppSlug),
		installations: groupByAccount(list?.repositories ?? []),
		fetchedAt: list?.fetchedAt.toISOString() ?? null,
		sandboxes: projects.map((p) => ({ name: p.name, displayName: p.displayName, ready: p.ready }))
	};
};
