import type { ChartRelease, ChartRollout } from '#lib/about-types.ts';

export const UPDATER_STATUS_TIMEOUT_MS = 2_000;
/** The updater Service the chart always creates (enzarb-updater, port 8080 by default). */
export const DEFAULT_UPDATER_STATUS_URL = 'http://enzarb-updater:8080/status';

const ts = (v: unknown) => typeof v === 'string' && !Number.isNaN(Date.parse(v));

/** Entries without a version or a valid time are dropped; anything but a list is empty. */
function parseReleases(v: unknown): ChartRelease[] {
	if (!Array.isArray(v)) return [];
	return v.flatMap((e: unknown) => {
		const r = (typeof e === 'object' && e !== null ? e : {}) as Record<string, unknown>;
		return typeof r.version === 'string' && r.version && ts(r.deployedAt)
			? [{ version: r.version, deployedAt: r.deployedAt as string }]
			: [];
	});
}

const str = (v: unknown) => (typeof v === 'string' && v ? v : null);

/**
 * The updater's chart rollout, or null when the updater is absent, unreachable, too old to have
 * `/status` or answers something unexpected. Never throws: the About page just omits it.
 */
export async function fetchChartRollout(
	opts: { url?: string; fetch?: typeof fetch } = {}
): Promise<ChartRollout | null> {
	const url = opts.url ?? (process.env.ENZARB_UPDATER_STATUS_URL || DEFAULT_UPDATER_STATUS_URL);
	try {
		const res = await (opts.fetch ?? fetch)(url, {
			signal: AbortSignal.timeout(UPDATER_STATUS_TIMEOUT_MS)
		});
		if (!res.ok) return null;
		const body: unknown = await res.json();
		if (typeof body !== 'object' || body === null) return null;
		const b = body as Record<string, unknown>;
		if (typeof b.stuck !== 'boolean') return null;
		return {
			installed: str(b.installed),
			desired: str(b.desired),
			latest: str(b.latest),
			stuck: b.stuck,
			releases: parseReleases(b.releases)
		};
	} catch {
		return null;
	}
}
