/**
 * Token usage and cost of agent jobs, as `workflows_api` exposes them (`job_v.usage`,
 * `work_item_v.usage_totals`, a SQL name; contracts §6). Every field is optional: absent means "not reported",
 * never 0. `costUsd` is the cost as reported by Claude, not an estimate.
 */
export interface Usage {
	inputTokens: number | null;
	outputTokens: number | null;
	cacheReadTokens: number | null;
	cacheWriteTokens: number | null;
	models: string[];
	costUsd: number | null;
}

const NONE = '—';

const num = (x: unknown): number | null =>
	typeof x === 'number' && Number.isFinite(x) && x >= 0 ? x : null;

/** Reads the jsonb the views return (enzarb-proto `Usage` shape). Null when nothing is in it. */
export function parseUsage(raw: unknown): Usage | null {
	if (typeof raw !== 'object' || raw === null) return null;
	const o = raw as Record<string, unknown>;
	const usage: Usage = {
		inputTokens: num(o.input_tokens),
		outputTokens: num(o.output_tokens),
		cacheReadTokens: num(o.cache_read_input_tokens),
		cacheWriteTokens: num(o.cache_creation_input_tokens),
		models: Array.isArray(o.models)
			? o.models.filter((m): m is string => typeof m === 'string' && m !== '')
			: [],
		costUsd: num(o.cost_usd)
	};
	return isEmpty(usage) ? null : usage;
}

function isEmpty(u: Usage): boolean {
	return (
		u.inputTokens === null &&
		u.outputTokens === null &&
		u.cacheReadTokens === null &&
		u.cacheWriteTokens === null &&
		u.models.length === 0 &&
		u.costUsd === null
	);
}

/** Compact count: `842`, `1.2k`, `12.3k`, `123k`, `1.2M`. Null (not reported) is a dash; 0 is `0`. */
export function formatTokens(n: number | null | undefined): string {
	if (n === null || n === undefined || !Number.isFinite(n)) return NONE;
	const v = Math.max(0, Math.round(n));
	if (v < 1000) return String(v);
	const trim = (x: number) => x.toFixed(1).replace(/\.0$/, '');
	if (v < 99_950) return `${trim(v / 1e3)}k`;
	if (v < 999_500) return `${Math.round(v / 1e3)}k`;
	return `${trim(v / 1e6)}M`;
}

/** `$0.42`; `<$0.01` below a cent; a dash when the cost was not reported (0 is `$0.00`). */
export function formatCost(usd: number | null | undefined): string {
	if (usd === null || usd === undefined || !Number.isFinite(usd)) return NONE;
	if (usd > 0 && usd < 0.005) return '<$0.01';
	return `$${usd.toFixed(2)}`;
}

/** Field-wise sum; a field no one reported stays null. Null when there is nothing to show. */
export function sumUsage(list: readonly (Usage | null | undefined)[]): Usage | null {
	const add = (a: number | null, b: number | null) => (a === null ? b : b === null ? a : a + b);
	let total: Usage = {
		inputTokens: null,
		outputTokens: null,
		cacheReadTokens: null,
		cacheWriteTokens: null,
		models: [],
		costUsd: null
	};
	for (const u of list) {
		if (!u) continue;
		total = {
			inputTokens: add(total.inputTokens, u.inputTokens),
			outputTokens: add(total.outputTokens, u.outputTokens),
			cacheReadTokens: add(total.cacheReadTokens, u.cacheReadTokens),
			cacheWriteTokens: add(total.cacheWriteTokens, u.cacheWriteTokens),
			models: [...new Set([...total.models, ...u.models])],
			costUsd: add(total.costUsd, u.costUsd)
		};
	}
	return isEmpty(total) ? null : total;
}

/**
 * Everything sent to the model: uncached input plus cache reads and writes (`inputTokens` alone is
 * only the uncached part). Null fields are skipped; null when none of the three is reported.
 */
export function totalInput(u: Usage | null | undefined): number | null {
	if (!u) return null;
	const parts = [u.inputTokens, u.cacheReadTokens, u.cacheWriteTokens].filter(
		(x): x is number => x !== null
	);
	return parts.length === 0 ? null : parts.reduce((a, b) => a + b, 0);
}

/** Tooltip text for the input breakdown, e.g. `Uncached 1.2k · Cache read 9k · Cache write 50`. */
export function cacheBreakdown(u: Usage | null | undefined): string {
	return `Uncached ${formatTokens(u?.inputTokens)} · Cache read ${formatTokens(u?.cacheReadTokens)} · Cache write ${formatTokens(u?.cacheWriteTokens)}`;
}
