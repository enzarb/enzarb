import type { ChartRollout } from '#lib/about-types.ts';

export const UPDATER_STATUS_TIMEOUT_MS = 2_000;
/** The updater Service the chart always creates (enzarb-updater, port 8080 by default). */
export const DEFAULT_UPDATER_STATUS_URL = 'http://enzarb-updater:8080/status';

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
			stuck: b.stuck
		};
	} catch {
		return null;
	}
}
