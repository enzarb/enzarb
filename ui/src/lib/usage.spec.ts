import { describe, expect, it } from 'vitest';
import {
	cacheBreakdown,
	formatCost,
	formatTokens,
	parseUsage,
	sumUsage,
	totalInput,
	type Usage
} from './usage.ts';

const u = (over: Partial<Usage> = {}): Usage => ({
	inputTokens: null,
	outputTokens: null,
	cacheReadTokens: null,
	cacheWriteTokens: null,
	models: [],
	costUsd: null,
	...over
});

describe('formatTokens', () => {
	it('is compact', () => {
		expect(formatTokens(0)).toBe('0');
		expect(formatTokens(842)).toBe('842');
		expect(formatTokens(1000)).toBe('1k');
		expect(formatTokens(1234)).toBe('1.2k');
		expect(formatTokens(12_345)).toBe('12.3k');
		expect(formatTokens(123_456)).toBe('123k');
		expect(formatTokens(999_499)).toBe('999k');
		expect(formatTokens(999_500)).toBe('1M');
		expect(formatTokens(1_234_567)).toBe('1.2M');
		expect(formatTokens(12_300_000)).toBe('12.3M');
	});

	it('shows a dash for unknown and a real zero for 0', () => {
		expect(formatTokens(null)).toBe('—');
		expect(formatTokens(undefined)).toBe('—');
		expect(formatTokens(Number.NaN)).toBe('—');
		expect(formatTokens(0)).not.toBe(formatTokens(null));
	});
});

describe('formatCost', () => {
	it('formats dollars with no estimate marker', () => {
		expect(formatCost(0.42)).toBe('$0.42');
		expect(formatCost(12)).toBe('$12.00');
		expect(formatCost(0)).toBe('$0.00');
		expect(formatCost(0.001)).toBe('<$0.01');
		expect(formatCost(0.42)).not.toMatch(/est/i);
	});

	it('shows a dash when the cost was not reported', () => {
		expect(formatCost(null)).toBe('—');
		expect(formatCost(undefined)).toBe('—');
	});
});

describe('parseUsage', () => {
	it('reads the view jsonb', () => {
		expect(
			parseUsage({
				input_tokens: 1200,
				output_tokens: 340,
				cache_read_input_tokens: 9000,
				models: ['m1', 'm2'],
				cost_usd: 0.42
			})
		).toEqual(
			u({
				inputTokens: 1200,
				outputTokens: 340,
				cacheReadTokens: 9000,
				models: ['m1', 'm2'],
				costUsd: 0.42
			})
		);
	});

	it('keeps absent fields null and a real 0 as 0', () => {
		expect(parseUsage({ input_tokens: 0 })).toEqual(u({ inputTokens: 0 }));
	});

	it('is null for nothing, and tolerates junk', () => {
		for (const raw of [null, undefined, {}, [], 'x', 3, { input_tokens: 'a', cost_usd: -1 }]) {
			expect(parseUsage(raw)).toBeNull();
		}
		expect(parseUsage({ models: ['a', 3, ''], input_tokens: 1 })?.models).toEqual(['a']);
	});
});

describe('sumUsage', () => {
	it('sums field by field and unions models', () => {
		const t = sumUsage([
			u({ inputTokens: 10, outputTokens: 1, models: ['a'], costUsd: 0.1 }),
			u({ inputTokens: 5, cacheReadTokens: 7, models: ['a', 'b'], costUsd: 0.2 })
		])!;
		expect(t.inputTokens).toBe(15);
		expect(t.outputTokens).toBe(1);
		expect(t.cacheReadTokens).toBe(7);
		expect(t.cacheWriteTokens).toBeNull();
		expect(t.models).toEqual(['a', 'b']);
		expect(t.costUsd).toBeCloseTo(0.3);
	});

	it('keeps unknown unknown: no job with a cost means no cost, not 0', () => {
		const t = sumUsage([u({ inputTokens: 1 }), null, undefined, u({ inputTokens: 2 })])!;
		expect(t.inputTokens).toBe(3);
		expect(t.costUsd).toBeNull();
		expect(formatCost(t.costUsd)).toBe('—');
	});

	it('only the jobs that reported a cost add to it', () => {
		const t = sumUsage([u({ inputTokens: 1, costUsd: 0.5 }), u({ inputTokens: 1 })])!;
		expect(t.costUsd).toBe(0.5);
	});

	it('is null when nothing was reported', () => {
		expect(sumUsage([])).toBeNull();
		expect(sumUsage([null, undefined, u()])).toBeNull();
	});

	it('treats 0 as a value', () => {
		expect(sumUsage([u({ inputTokens: 0 })])?.inputTokens).toBe(0);
	});
});

describe('totalInput', () => {
	it('adds uncached input, cache read and cache write', () => {
		expect(totalInput(u({ inputTokens: 1200, cacheReadTokens: 9000, cacheWriteTokens: 50 }))).toBe(
			10_250
		);
	});
	it('skips fields that were not reported', () => {
		expect(totalInput(u({ cacheReadTokens: 9000 }))).toBe(9000);
		expect(totalInput(u({ inputTokens: 0 }))).toBe(0);
	});
	it('is null when nothing is reported', () => {
		expect(totalInput(u({ outputTokens: 5 }))).toBeNull();
		expect(totalInput(null)).toBeNull();
	});
});

describe('cacheBreakdown', () => {
	it('names uncached, read and write, with dashes for unknown', () => {
		expect(
			cacheBreakdown(u({ inputTokens: 1200, cacheReadTokens: 9000, cacheWriteTokens: 50 }))
		).toBe('Uncached 1.2k · Cache read 9k · Cache write 50');
		expect(cacheBreakdown(u({ cacheReadTokens: 9000 }))).toBe(
			'Uncached — · Cache read 9k · Cache write —'
		);
		expect(cacheBreakdown(null)).toBe('Uncached — · Cache read — · Cache write —');
	});
});
