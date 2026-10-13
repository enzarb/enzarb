import { getConfig } from '#lib/server/config.ts';
import { getSql } from '#lib/server/db.ts';
import { hasUserRepositories, installUrl } from '#lib/server/user-repositories.ts';
import type { LayoutServerLoad } from './$types';

export const load: LayoutServerLoad = async ({ locals }) => {
	const installed = locals.user ? await hasUserRepositories(getSql(), locals.user.id) : true;
	return {
		installBanner: installed ? null : { installUrl: installUrl(getConfig().githubAppSlug) }
	};
};
