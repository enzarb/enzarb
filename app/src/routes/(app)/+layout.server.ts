import { redirect } from '@sveltejs/kit';
import { listProjects, purgeAfterOf } from '$lib/k8s';
import type { LayoutServerLoad } from './$types';

export const load: LayoutServerLoad = async ({ locals, url }) => {
	if (!locals.session) {
		redirect(302, `/auth/login?returnTo=${encodeURIComponent(url.pathname)}`);
	}
	const orgProjects = Object.fromEntries(
		await Promise.all(
			locals.session.orgs.map(async (org) => {
				const all = await listProjects(org.id);
				const projects = all
					.filter((p: any) => !purgeAfterOf(p))
					.map((p: any) => ({
						slug: p.metadata?.name as string,
						displayName: (p.spec?.displayName ?? p.metadata?.name) as string,
					}));
				return [org.slug, projects] as const;
			})
		)
	);
	return { session: locals.session, orgProjects };
};
