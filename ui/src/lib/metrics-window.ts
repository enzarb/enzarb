/**
 * The time window of the project Metrics tab. Pure, so the page and the server parse the same URL
 * the same way: `?window=24h|7d|30d`, or `?from=&to=` (ISO date or datetime). The window is
 * half-open, `[from, to)`, like `workflows_api.project_usage`.
 */
export const PRESETS = ['24h', '7d', '30d'] as const;
export type Preset = (typeof PRESETS)[number];

export const DEFAULT_PRESET: Preset = '7d';
/** Longest span a window may cover. */
export const MAX_SPAN_DAYS = 366;
/** How far past `now` a `to` may be (clock skew between browser and server). */
export const FUTURE_SKEW_MS = 5 * 60_000;

const HOUR = 3_600_000;
const DAY = 24 * HOUR;
const PRESET_MS: Record<Preset, number> = { '24h': DAY, '7d': 7 * DAY, '30d': 30 * DAY };

export interface MetricsWindow {
	from: Date;
	to: Date;
	preset: Preset | 'custom';
}

export type WindowErrorCode = 'preset' | 'missing' | 'unparsable' | 'order' | 'span' | 'future';

export interface WindowError {
	code: WindowErrorCode;
	message: string;
}

export type WindowResult = { ok: true; window: MetricsWindow } | { ok: false; error: WindowError };

/** The default window (last 7 days), for when the URL has none or an invalid one. */
export const defaultWindow = (now: Date): MetricsWindow => ({
	from: new Date(now.getTime() - PRESET_MS[DEFAULT_PRESET]),
	to: now,
	preset: DEFAULT_PRESET
});

const fail = (code: WindowErrorCode, message: string): WindowResult => ({
	ok: false,
	error: { code, message }
});

const DATE_ONLY = /^\d{4}-\d{2}-\d{2}$/;
const DATE_TIME = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:\d{2})$/;

interface Instant {
	at: Date;
	dateOnly: boolean;
}

/** ISO date (a UTC day) or datetime with an offset; null when it is neither or not a real date. */
function parseInstant(raw: string): Instant | null {
	const dateOnly = DATE_ONLY.test(raw);
	if (!dateOnly && !DATE_TIME.test(raw)) return null;
	const at = new Date(dateOnly ? `${raw}T00:00:00Z` : raw);
	if (Number.isNaN(at.getTime())) return null;
	// Rejects overflowing days such as 2026-02-31, which `Date` would roll over silently.
	if (dateOnly && at.toISOString().slice(0, 10) !== raw) return null;
	return { at, dateOnly };
}

/**
 * Reads the window from URL params; `now` anchors the presets. No params means the default
 * preset. A date-only `to` covers that whole UTC day.
 */
export function parseWindow(params: Pick<URLSearchParams, 'get'>, now: Date): WindowResult {
	const preset = params.get('window');
	if (preset !== null) {
		if (!(PRESETS as readonly string[]).includes(preset))
			return fail('preset', `Unknown window "${preset}"; use 24h, 7d or 30d.`);
		const p = preset as Preset;
		return {
			ok: true,
			window: { from: new Date(now.getTime() - PRESET_MS[p]), to: now, preset: p }
		};
	}
	const rawFrom = params.get('from');
	const rawTo = params.get('to');
	if (rawFrom === null && rawTo === null) {
		return parseWindow(new URLSearchParams({ window: DEFAULT_PRESET }), now);
	}
	if (!rawFrom || !rawTo) return fail('missing', 'Set both a start and an end.');
	const from = parseInstant(rawFrom);
	const to = parseInstant(rawTo);
	if (!from || !to)
		return fail('unparsable', 'Dates must look like 2026-10-01 or 2026-10-01T08:00Z.');
	const end = to.dateOnly ? new Date(to.at.getTime() + DAY) : to.at;
	if (from.at.getTime() >= end.getTime()) return fail('order', 'The start must be before the end.');
	if (end.getTime() - from.at.getTime() > MAX_SPAN_DAYS * DAY)
		return fail('span', `A window can cover at most ${MAX_SPAN_DAYS} days.`);
	// A date-only end is a whole day: it only has to have started.
	if (to.at.getTime() > now.getTime() + FUTURE_SKEW_MS)
		return fail('future', 'The end cannot be in the future.');
	return { ok: true, window: { from: from.at, to: end, preset: 'custom' } };
}

const isMidnight = (d: Date) => d.getTime() % DAY === 0;

/** The inverse of `parseWindow`: `?window=7d`, or `?from=&to=` (dates when both end on midnight UTC). */
export function windowToParams(w: MetricsWindow): URLSearchParams {
	if (w.preset !== 'custom') return new URLSearchParams({ window: w.preset });
	if (isMidnight(w.from) && isMidnight(w.to)) {
		const lastDay = new Date(w.to.getTime() - DAY);
		return new URLSearchParams({
			from: w.from.toISOString().slice(0, 10),
			to: lastDay.toISOString().slice(0, 10)
		});
	}
	return new URLSearchParams({ from: w.from.toISOString(), to: w.to.toISOString() });
}

/** The parse result plus the window to query: the parsed one, or the default when the URL is invalid. */
export function resolveWindow(params: Pick<URLSearchParams, 'get'>, now: Date) {
	const parsed = parseWindow(params, now);
	return { parsed, current: parsed.ok ? parsed.window : defaultWindow(now) };
}
