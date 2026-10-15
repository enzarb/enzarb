import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { UsageTotal } from '#lib/server/metrics-service.ts';
import ProjectUsageSummary from './ProjectUsageSummary.svelte';

const none = { reporters: 0, avg: null, median: null };
const total = (over: Partial<UsageTotal> = {}): UsageTotal => ({
	items: 2,
	usage: null,
	input: none,
	output: none,
	cost: none,
	...over
});

const text = async (t: UsageTotal) =>
	(await render(ProjectUsageSummary, { total: t })).container.textContent ?? '';

describe('ProjectUsageSummary', () => {
	it('shows a dash, not 0, for what was not reported', async () => {
		const s = await text(total());
		expect(s).toContain('—');
		expect(s).not.toContain('$0.00');
		expect(s).not.toMatch(/Output\s*0/);
		expect(s).toContain('0 of 2 tasks');
	});

	it('shows a reported zero as zero', async () => {
		const s = await text(
			total({
				usage: {
					inputTokens: 0,
					outputTokens: 0,
					cacheReadTokens: null,
					cacheWriteTokens: null,
					models: [],
					costUsd: 0
				},
				cost: { reporters: 1, avg: 0, median: 0 }
			})
		);
		expect(s).toContain('$0.00');
		expect(s).toMatch(/Output\s*0/);
	});

	it('formats real figures', async () => {
		const s = await text(
			total({
				usage: {
					inputTokens: 1200,
					outputTokens: 500,
					cacheReadTokens: 0,
					cacheWriteTokens: null,
					models: [],
					costUsd: 1.5
				},
				cost: { reporters: 2, avg: 0.75, median: 0.75 }
			})
		);
		expect(s).toContain('1.2k');
		expect(s).toContain('$1.50');
		expect(s).toContain('2 of 2 tasks');
	});
});
