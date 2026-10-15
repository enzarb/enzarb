<script lang="ts">
	import type { SandboxStats } from '#lib/server/metrics-service.ts';
	import { formatBytes, formatBytesPerSec, formatCores } from '#lib/metrics-format.ts';
	import SeriesChart, { type Line } from './SeriesChart.svelte';
	import EmptyState from './ui/EmptyState.svelte';

	/** CPU, memory, used storage and disk I/O of the project's sandboxes, from `sandbox_stats_window`. */
	let { stats }: { stats: SandboxStats } = $props();

	type Pick = (b: SandboxStats['buckets'][number]) => number | null;
	const line = (label: string, pick: Pick, reference = false): Line => ({
		label,
		reference,
		points: stats.buckets.map((b) => ({ t: b.t, v: pick(b) }))
	});

	const charts = $derived([
		{
			title: 'CPU',
			format: formatCores,
			lines: [line('Average', (b) => b.cpuAvg), line('Peak', (b) => b.cpuMax)]
		},
		{
			title: 'Memory',
			format: formatBytes,
			lines: [line('Average', (b) => b.memAvg), line('Peak', (b) => b.memMax)]
		},
		{
			title: 'Storage used',
			format: formatBytes,
			lines: [line('Used', (b) => b.storageUsedAvg), line('Total', (b) => b.storageTotal, true)]
		},
		{
			title: 'Disk I/O',
			format: formatBytesPerSec,
			lines: [line('Read', (b) => b.readAvg), line('Write', (b) => b.writeAvg)]
		}
	]);
</script>

{#if stats.buckets.length === 0}
	<EmptyState
		level={3}
		title="No sandbox utilization in this window"
		description="No samples were recorded. Older sandboxes, or sandboxes started before sampling was rolled out, report nothing."
	/>
{:else}
	<div class="ez-utilization" data-testid="sandbox-utilization">
		{#each charts as c (c.title)}
			<SeriesChart
				title={c.title}
				format={c.format}
				lines={c.lines}
				from={stats.from}
				to={stats.to}
				stepMs={stats.stepMs}
			/>
		{/each}
	</div>
{/if}
