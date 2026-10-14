const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** Past this age a relative time stops being useful and the date is shown. */
const MAX_RELATIVE_DAYS = 30;

/**
 * "just now", "5m ago", "3h ago", "2d ago", or the date (`2026-09-01`) after 30 days. `iso` is an
 * ISO timestamp; a timestamp in the future (clock skew) reads as "just now", and one that cannot
 * be parsed as "unknown".
 */
export function timeSince(iso: string, now: number = Date.now()): string {
	const at = Date.parse(iso);
	if (!Number.isFinite(at)) return 'unknown';
	const ms = now - at;
	if (ms < MINUTE) return 'just now';
	if (ms < HOUR) return `${Math.floor(ms / MINUTE)}m ago`;
	if (ms < DAY) return `${Math.floor(ms / HOUR)}h ago`;
	const days = Math.floor(ms / DAY);
	if (days < MAX_RELATIVE_DAYS) return `${days}d ago`;
	return new Date(at).toISOString().slice(0, 10);
}
