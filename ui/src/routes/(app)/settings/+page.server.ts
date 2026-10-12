import { redirect } from '@sveltejs/kit';
import { getConfig } from '#lib/server/config.ts';
import { getSql } from '#lib/server/db.ts';
import {
	getUserRepositorySnapshot,
	groupByAccount,
	installUrl
} from '#lib/server/user-repositories.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals }) => {
	if (!locals.user) redirect(303, '/login');
	const snapshot = await getUserRepositorySnapshot(getSql(), locals.user.id);
	return {
		account: { login: locals.user.login, name: locals.user.name },
		installUrl: installUrl(getConfig().githubAppSlug),
		installations: groupByAccount(snapshot.repositories),
		fetchedAt: snapshot.fetchedAt?.toISOString() ?? null
	};
};
