import { redirect } from '@sveltejs/kit';
import type { AboutComponent, AboutRelease } from '#lib/about-types.ts';
import { getConfig } from '#lib/server/config.ts';
import { getInstallationToken, invalidateInstallationToken } from '#lib/server/github-app-token.ts';
import { renderMarkdown } from '#lib/server/markdown.ts';
import { cachedReleases } from '#lib/server/releases.ts';
import { fetchChartRollout } from '#lib/server/updater-status.ts';
import { COMPONENTS, displayVersion, componentDirUrl } from '#lib/versions.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals }) => {
	if (!locals.user) redirect(303, '/login');
	const { chart, components: deployed } = getConfig().componentVersions;

	const known = COMPONENTS.map((c) => ({ ...c, repoUrl: componentDirUrl(c.dir) as string | null }));
	const knownNames = new Set(COMPONENTS.map((c) => c.name));
	const extras = Object.keys(deployed)
		.filter((name) => !knownNames.has(name))
		.sort()
		.map((name) => ({ name, label: name, dir: null, repoUrl: null }));

	// Server-side only: the token is never put in the returned page data.
	const token = (await getInstallationToken().catch(() => null)) ?? undefined;
	const releaseOpts = { token, onTokenRejected: invalidateInstallationToken };

	// Started now so it overlaps the GitHub release fetches; null when the updater is unavailable.
	const chartRolloutPromise = fetchChartRollout();

	const components: AboutComponent[] = await Promise.all(
		[...known, ...extras].map(async (c) => {
			let releases: AboutRelease[] | null = null;
			if (c.dir) {
				try {
					releases = (await cachedReleases(c.dir, releaseOpts)).map((r) => ({
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
	return { chart: displayVersion(chart), chartRollout: await chartRolloutPromise, components };
};
