import { getConfig } from '#lib/server/config.ts';
import { getRepositoryAccess } from '#lib/server/repo-access.ts';
import { installUrl } from '#lib/server/user-repositories.ts';
import type { LayoutServerLoad } from './$types';

/** Whether the install notice is due: the live list (display cache) holds nothing for the user. */
async function installBanner(login: string): Promise<{ installUrl: string } | null> {
	try {
		const { repositories } = await getRepositoryAccess().list(login);
		if (repositories.length > 0) return null;
	} catch {
		// GitHub unreachable: say nothing rather than claim the App is missing.
		return null;
	}
	return { installUrl: installUrl(getConfig().githubAppSlug) };
}

export const load: LayoutServerLoad = async ({ locals }) => ({
	// Streamed: listing can take a moment and must not hold up the page.
	installBanner: locals.user ? installBanner(locals.user.login) : Promise.resolve(null)
});
