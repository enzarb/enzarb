/** "–" for a duration that was not measured. */
const NONE = '–';

/**
 * A duration in seconds as "850ms", "42s", "3m 05s", "2h 10m" or "1d 4h" (two units at most);
 * null, negative or non-finite reads as a dash.
 */
export function formatDuration(seconds: number | null | undefined): string {
	if (seconds === null || seconds === undefined || !Number.isFinite(seconds) || seconds < 0)
		return NONE;
	if (seconds < 1) return `${Math.round(seconds * 1000)}ms`;
	const s = Math.round(seconds);
	if (s < 60) return `${s}s`;
	const m = Math.floor(s / 60);
	if (m < 60) return `${m}m ${String(s % 60).padStart(2, '0')}s`;
	const h = Math.floor(m / 60);
	if (h < 24) return `${h}h ${String(m % 60).padStart(2, '0')}m`;
	return `${Math.floor(h / 24)}d ${h % 24}h`;
}
