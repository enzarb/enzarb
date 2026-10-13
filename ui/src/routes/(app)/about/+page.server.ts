import { redirect } from '@sveltejs/kit';
import type { AboutComponent, AboutRelease } from '#lib/about-types.ts';
import { getConfig } from '#lib/server/config.ts';
import { renderMarkdown } from '#lib/server/markdown.ts';
import { cachedReleases } from '#lib/server/releases.ts';
import { COMPONENTS, displayVersion, githubRepoUrl } from '#lib/versions.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals }) => {
	if (!locals.user) redirect(303, '/login');
	const { chart, components: deployed } = getConfig().componentVersions;

	const known = COMPONENTS.map((c) => ({ ...c, repoUrl: githubRepoUrl(c.repo) as string | null }));
	const knownNames = new Set(COMPONENTS.map((c) => c.name));
	const extras = Object.keys(deployed)
		.filter((name) => !knownNames.has(name))
		.sort()
		.map((name) => ({ name, label: name, repo: null, repoUrl: null }));

	const components: AboutComponent[] = await Promise.all(
		[...known, ...extras].map(async (c) => {
			let releases: AboutRelease[] | null = null;
			if (c.repo) {
				try {
					releases = (await cachedReleases(c.repo)).map((r) => ({
						tag: r.tag,
						name: r.name,
						publishedAt: r.publishedAt,
						url: r.url,
						html: renderMarkdown(r.body)
					}));
				} catch {
					// GitHub unreachable or rate-limited: the version is still shown.
				}
			}
			return {
				name: c.name,
				label: c.label,
				repoUrl: c.repoUrl,
				deployed: displayVersion(deployed[c.name]),
				releases
			};
		})
	);
	return { chart: displayVersion(chart), components };
};
