import { parseUsage, type Usage } from '#lib/usage.ts';
import type { MetricsWindow } from '#lib/metrics-window.ts';
import type { Sql } from './db.ts';

/** A statistic over the items that reported a field; null when none did. */
export interface FieldStats {
	reporters: number;
	avg: number | null;
	median: number | null;
}

export interface UsageTotal {
	items: number;
	usage: Usage | null;
	input: FieldStats;
	output: FieldStats;
	cost: FieldStats;
}

export interface TaskUsage {
	key: string;
	state: string;
	usage: Usage | null;
}

export interface ProjectUsage {
	total: UsageTotal;
	tasks: TaskUsage[];
}

/** A row of `workflows_api.project_usage` (contracts §6). numeric/bigint arrive as strings. */
export interface UsageDbRow {
	kind: string;
	work_item_key: string | null;
	state: string | null;
	items: unknown;
	input_tokens: unknown;
	output_tokens: unknown;
	cache_read_input_tokens: unknown;
	cache_creation_input_tokens: unknown;
	cost_usd: unknown;
	input_tokens_reporters: unknown;
	output_tokens_reporters: unknown;
	cost_usd_reporters: unknown;
	input_tokens_avg: unknown;
	output_tokens_avg: unknown;
	cost_usd_avg: unknown;
	input_tokens_median: unknown;
	output_tokens_median: unknown;
	cost_usd_median: unknown;
}

/** A finite, non-negative number from a number or numeric string; null (not reported) otherwise, never 0. */
export function numeric(x: unknown): number | null {
	const n = typeof x === 'string' && x.trim() !== '' ? Number(x) : x;
	return typeof n === 'number' && Number.isFinite(n) && n >= 0 ? n : null;
}

const count = (x: unknown) => numeric(x) ?? 0;

function usageOf(r: UsageDbRow): Usage | null {
	return parseUsage({
		input_tokens: numeric(r.input_tokens),
		output_tokens: numeric(r.output_tokens),
		cache_read_input_tokens: numeric(r.cache_read_input_tokens),
		cache_creation_input_tokens: numeric(r.cache_creation_input_tokens),
		cost_usd: numeric(r.cost_usd)
	});
}

const stats = (reporters: unknown, avg: unknown, median: unknown): FieldStats => ({
	reporters: count(reporters),
	avg: numeric(avg),
	median: numeric(median)
});

/** Maps the function's rows; a missing `total` row reads as an empty window. */
export function mapUsageRows(rows: readonly UsageDbRow[]): ProjectUsage {
	const total = rows.find((r) => r.kind === 'total');
	return {
		total: {
			items: count(total?.items),
			usage: total ? usageOf(total) : null,
			input: stats(
				total?.input_tokens_reporters,
				total?.input_tokens_avg,
				total?.input_tokens_median
			),
			output: stats(
				total?.output_tokens_reporters,
				total?.output_tokens_avg,
				total?.output_tokens_median
			),
			cost: stats(total?.cost_usd_reporters, total?.cost_usd_avg, total?.cost_usd_median)
		},
		tasks: rows
			.filter((r) => r.kind === 'task' && typeof r.work_item_key === 'string')
			.map((r) => ({ key: r.work_item_key!, state: r.state ?? '', usage: usageOf(r) }))
	};
}

/** Usage of the work items `project` created in `window`. The caller has checked ownership. */
export async function projectUsage(
	sql: Sql,
	project: string,
	window: Pick<MetricsWindow, 'from' | 'to'>
): Promise<ProjectUsage> {
	const rows = await sql<UsageDbRow[]>`
		SELECT * FROM workflows_api.project_usage(
			${project}, ${window.from.toISOString()}::timestamptz, ${window.to.toISOString()}::timestamptz)`;
	return mapUsageRows(rows);
}
