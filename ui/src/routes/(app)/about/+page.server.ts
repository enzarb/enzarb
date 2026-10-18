import { redirect } from '@sveltejs/kit';
import type {
	AboutComponent,
	AboutRelease,
	ChartRelease,
	ReleaseTiming
} from '#lib/about-types.ts';
import { getProjectStore } from '#lib/server/kube.ts';
import { getConfig } from '#lib/server/config.ts';
import { invalidateToken, token as appToken } from '#lib/server/github-app.ts';
import { renderMarkdown } from '#lib/server/markdown.ts';
import { RELEASES_REPO, cachedReleases } from '#lib/server/releases.ts';
import { fetchChartRollout } from '#lib/server/updater-status.ts';
import { releaseTimings } from '#lib/release-timing.ts';
import type { LastRoll } from '#lib/project-types.ts';
import { COMPONENTS, displayVersion, componentDirUrl, normalizeTag } from '#lib/versions.ts';
import type { PageServerLoad } from './$types';

// Releases are public: the token exists for rate limits, not access.
const RELEASES_PERMS = { contents: 'read' } as const;

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
	const token = (await appToken(RELEASES_REPO, RELEASES_PERMS).catch(() => null)) ?? undefined;
	const releaseOpts = {
		token,
		onTokenRejected: () => invalidateToken(RELEASES_REPO, RELEASES_PERMS)
	};

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
	const chartRollout = await chartRolloutPromise;
	const timings = await recentTimings(chartRollout?.releases ?? [], releaseOpts);
	return { chart: displayVersion(chart), chartRollout, components, releaseTimings: timings };
};

/** Merge-to-deployed of the last chart releases. A failed source just leaves its stage empty. */
async function recentTimings(
	deployed: ChartRelease[],
	releaseOpts: Parameters<typeof cachedReleases>[1]
): Promise<ReleaseTiming[]> {
	const [chartReleases, projects] = await Promise.all([
		cachedReleases('helm', releaseOpts).catch(() => []),
		getProjectStore()
			.list()
			.catch(() => [])
	]);
	const rolls = projects.flatMap((p) =>
		p.status?.lastRoll ? [p.status.lastRoll as LastRoll] : []
	);
	return releaseTimings(
		chartReleases.map((r) => ({
			version: r.tag,
			url: r.url,
			publishedAt: r.publishedAt,
			sandboxImageVersion: deployed.find((d) => normalizeTag(d.version) === normalizeTag(r.tag))
				?.sandboxImageVersion
		})),
		deployed,
		rolls
	);
}
