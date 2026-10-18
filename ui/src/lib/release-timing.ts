import type { ChartRelease, ReleaseTiming } from '#lib/about-types.ts';
import type { LastRoll } from '#lib/project-types.ts';
import { normalizeTag } from '#lib/versions.ts';

export const RECENT_RELEASES = 5;

export interface TimedRelease {
	/** Chart tag, e.g. `v0.9.0`. */
	version: string;
	url: string;
	publishedAt: string | null;
	/** The sandbox image tag this chart release ships, when known. */
	sandboxImageVersion?: string;
}

/** The tag of an image reference (`registry/name:1.2.3@sha256:...`), without a leading `v`. */
export function imageTag(image: string | undefined): string | null {
	if (!image) return null;
	const ref = image.split('@')[0];
	const colon = ref.lastIndexOf(':');
	if (colon <= ref.lastIndexOf('/')) return null;
	const tag = normalizeTag(ref.slice(colon + 1));
	return tag || null;
}

const ms = (t: string | null | undefined) => {
	const n = t ? Date.parse(t) : NaN;
	return Number.isNaN(n) ? null : n;
};

/**
 * Merge-to-deployed timing of the last `limit` chart releases. `releases` is newest first.
 *
 * Merge is the release's `publishedAt`; deployed comes from the updater (`deployed`, by version);
 * sandbox applied is the earliest roll that belongs to the release:
 * 1. by version, when a roll's image tag is the sandbox image a listed release ships;
 * 2. otherwise (no usable image, or an image no listed release claims) by time window: after the
 *    release's deploy and before the next newer deployed release.
 * Several Projects roll at different times; the earliest is shown.
 */
export function releaseTimings(
	releases: readonly TimedRelease[],
	deployed: readonly ChartRelease[],
	rolls: readonly LastRoll[],
	limit = RECENT_RELEASES
): ReleaseTiming[] {
	const deployedAt = new Map(deployed.map((d) => [normalizeTag(d.version), ms(d.deployedAt)]));
	const claimed = new Set(
		releases.flatMap((r) => (r.sandboxImageVersion ? [normalizeTag(r.sandboxImageVersion)] : []))
	);
	const dated = rolls.flatMap((r) => {
		const at = ms(r.at);
		return at === null ? [] : [{ at, tag: imageTag(r.image) }];
	});
	const earliest = (xs: number[]) => (xs.length ? Math.min(...xs) : null);

	return releases.slice(0, limit).map((r, i) => {
		const deployedMs = deployedAt.get(normalizeTag(r.version)) ?? null;
		const sandboxMs = (() => {
			const want = r.sandboxImageVersion ? normalizeTag(r.sandboxImageVersion) : null;
			const byVersion = want ? dated.filter((x) => x.tag === want).map((x) => x.at) : [];
			if (byVersion.length) return earliest(byVersion);
			if (deployedMs === null) return null;
			const next = releases
				.slice(0, i)
				.map((n) => deployedAt.get(normalizeTag(n.version)) ?? null)
				.filter((t): t is number => t !== null && t > deployedMs);
			const until = next.length ? Math.min(...next) : Infinity;
			return earliest(
				dated
					.filter((x) => !(x.tag && claimed.has(x.tag)) && x.at >= deployedMs && x.at < until)
					.map((x) => x.at)
			);
		})();
		const publishedMs = ms(r.publishedAt);
		const delta =
			publishedMs !== null && deployedMs !== null ? (deployedMs - publishedMs) / 1000 : null;
		const afterDeploy =
			sandboxMs !== null && deployedMs !== null ? (sandboxMs - deployedMs) / 1000 : null;
		const iso = (t: number | null) => (t === null ? null : new Date(t).toISOString());
		return {
			version: r.version,
			url: r.url,
			publishedAt: r.publishedAt,
			deployedAt: iso(deployedMs),
			sandboxAppliedAt: iso(sandboxMs),
			mergeToDeployedSeconds: delta !== null && delta >= 0 ? delta : null,
			skew: delta !== null && delta < 0,
			deployToSandboxSeconds: afterDeploy !== null && afterDeploy >= 0 ? afterDeploy : null
		};
	});
}
