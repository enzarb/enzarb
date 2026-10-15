import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { SandboxBucket } from '#lib/server/metrics-service.ts';
import SandboxUtilization from './SandboxUtilization.svelte';

const bucket = (t: number, over: Partial<SandboxBucket> = {}): SandboxBucket => ({
	t,
	samples: 3,
	cpuAvg: 0.5,
	cpuMax: 1,
	memAvg: 1024 ** 3,
	memMax: 2 * 1024 ** 3,
	readAvg: 2048,
	readMax: 4096,
	writeAvg: 1024,
	writeMax: 2048,
	storageUsedAvg: 5 * 1024 ** 3,
	storageUsedMax: 6 * 1024 ** 3,
	storageTotal: 20 * 1024 ** 3,
	...over
});

describe('SandboxUtilization', () => {
	it('explains an empty window instead of drawing charts', async () => {
		const screen = await render(SandboxUtilization, {
			stats: { from: 0, to: 100, stepMs: 1, buckets: [] }
		});
		await expect.element(page.getByText('No sandbox utilization in this window')).toBeVisible();
		expect(screen.container.querySelectorAll('svg[role="img"]').length).toBe(0);
	});

	it('draws CPU, memory, storage and disk I/O charts with formatted units', async () => {
		const screen = await render(SandboxUtilization, {
			stats: { from: 0, to: 1000, stepMs: 10, buckets: [bucket(0), bucket(10)] }
		});
		for (const name of ['CPU', 'Memory', 'Storage used', 'Disk I/O']) {
			await expect.element(page.getByRole('img', { name, exact: true })).toBeInTheDocument();
		}
		const text = screen.container.textContent;
		expect(text).toContain('0.5 cores');
		expect(text).toContain('1 GiB');
		expect(text).toContain('20 GiB');
		expect(text).toContain('2 KiB/s');
	});

	it('shows a NULL field as not reported', async () => {
		const screen = await render(SandboxUtilization, {
			stats: {
				from: 0,
				to: 1000,
				stepMs: 10,
				buckets: [bucket(0, { readAvg: null, writeAvg: null })]
			}
		});
		expect(screen.container.textContent).toContain('not reported');
	});
});
