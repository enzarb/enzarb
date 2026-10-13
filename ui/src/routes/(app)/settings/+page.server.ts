import { redirect } from '@sveltejs/kit';
import { getConfig } from '#lib/server/config.ts';
import { getSql } from '#lib/server/db.ts';
import { getProjectStore } from '#lib/server/kube.ts';
import { projectService } from '#lib/server/project-service.ts';
import {
	getUserRepositorySnapshot,
	groupByAccount,
	installUrl
} from '#lib/server/user-repositories.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals }) => {
	if (!locals.user) redirect(303, '/login');
	const [snapshot, projects] = await Promise.all([
		getUserRepositorySnapshot(getSql(), locals.user.id),
		// Claude setup runs in one of the user's own sandboxes; a cluster failure only hides the picker.
		projectService(getProjectStore(), locals.user.login)
			.list()
			.catch(() => [])
	]);
	return {
		account: { login: locals.user.login, name: locals.user.name },
		installUrl: installUrl(getConfig().githubAppSlug),
		installations: groupByAccount(snapshot.repositories),
		fetchedAt: snapshot.fetchedAt?.toISOString() ?? null,
		sandboxes: projects.map((p) => ({ name: p.name, displayName: p.displayName, ready: p.ready }))
	};
};
