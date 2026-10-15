import { describe, expect, it, vi } from 'vitest';
import type { Sql } from './db.ts';
import { mapUsageRows, numeric, projectUsage, type UsageDbRow } from './metrics-service.ts';

const row = (over: Partial<UsageDbRow>): UsageDbRow => ({
	kind: 'task',
	work_item_key: null,
	state: null,
	items: '1',
	input_tokens: null,
	output_tokens: null,
	cache_read_input_tokens: null,
	cache_creation_input_tokens: null,
	cost_usd: null,
	input_tokens_reporters: null,
	output_tokens_reporters: null,
	cost_usd_reporters: null,
	input_tokens_avg: null,
	output_tokens_avg: null,
	cost_usd_avg: null,
	input_tokens_median: null,
	output_tokens_median: null,
	cost_usd_median: null,
	...over
});

describe('numeric', () => {
	it('reads numbers and numeric strings', () => {
		expect(numeric(0)).toBe(0);
		expect(numeric('12.50')).toBe(12.5);
		expect(numeric('0')).toBe(0);
	});
	it.each([null, undefined, '', ' ', 'abc', NaN, Infinity, -1, '-3', {}])(
		'%s is not reported',
		(x) => {
			expect(numeric(x)).toBeNull();
		}
	);
});

describe('mapUsageRows', () => {
	it('reads an empty result as an empty window', () => {
		const r = mapUsageRows([]);
		expect(r.tasks).toEqual([]);
		expect(r.total.items).toBe(0);
		expect(r.total.usage).toBeNull();
		expect(r.total.cost).toEqual({ reporters: 0, avg: null, median: null });
	});

	it('maps the total row and keeps unreported fields null', () => {
		const r = mapUsageRows([
			row({
				kind: 'total',
				items: '3',
				input_tokens: '1200',
				cost_usd: '0.42',
				cost_usd_reporters: '2',
				cost_usd_avg: '0.21',
				cost_usd_median: 0.21
			})
		]);
		expect(r.total.items).toBe(3);
		expect(r.total.usage).toMatchObject({ inputTokens: 1200, outputTokens: null, costUsd: 0.42 });
		expect(r.total.cost).toEqual({ reporters: 2, avg: 0.21, median: 0.21 });
		expect(r.total.output).toEqual({ reporters: 0, avg: null, median: null });
	});

	it('turns missing, negative and NaN values into null, never 0', () => {
		const r = mapUsageRows([
			row({
				work_item_key: 'a#1',
				state: 'Done',
				input_tokens: '-5',
				output_tokens: 'NaN',
				cost_usd: undefined
			}),
			row({ work_item_key: 'a#2', state: 'Done', cost_usd: '0' })
		]);
		expect(r.tasks[0]).toEqual({ key: 'a#1', state: 'Done', usage: null });
		expect(r.tasks[1].usage?.costUsd).toBe(0);
		expect(r.tasks[1].usage?.inputTokens).toBeNull();
	});

	it('ignores task rows without a key', () => {
		expect(mapUsageRows([row({ work_item_key: null, cost_usd: '1' })]).tasks).toEqual([]);
	});
});

describe('projectUsage', () => {
	it('binds the project and the ISO window, and maps the rows', async () => {
		const sql = vi.fn(async () => [row({ kind: 'total', items: '0' })]);
		const from = new Date('2026-10-08T00:00:00Z');
		const to = new Date('2026-10-15T00:00:00Z');
		const r = await projectUsage(sql as unknown as Sql, 'alpha', { from, to });
		const [strings, ...values] = sql.mock.calls[0] as unknown as [
			TemplateStringsArray,
			...unknown[]
		];
		expect(strings.join('?')).toContain('workflows_api.project_usage(');
		expect(values).toEqual(['alpha', from.toISOString(), to.toISOString()]);
		expect(r.total.items).toBe(0);
	});
});
