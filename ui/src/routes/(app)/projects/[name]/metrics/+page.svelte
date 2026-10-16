<script lang="ts">
	import { page } from '$app/state';
	import BarChart from '#lib/components/BarChart.svelte';
	import StepMetricsTable from '#lib/components/tasks/StepMetricsTable.svelte';
	import EmptyState from '#lib/components/ui/EmptyState.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { getStepMetrics } from '#lib/metrics.remote.ts';
	import { resolveWindow, windowToParams } from '#lib/metrics-window.ts';
	import { formatCost } from '#lib/usage.ts';

	const current = $derived(resolveWindow(page.url.searchParams, new Date()).current);
	const metrics = $derived(
		getStepMetrics({ name: page.params.name!, ...Object.fromEntries(windowToParams(current)) })
	);
</script>

{#if metrics.error && !metrics.current}
	<Notice tone="error" title="Could not load metrics"
		>Something went wrong while loading metrics.</Notice
	>
{:else if !metrics.current}
	<Skeleton label="Loading metrics" rows={3} />
{:else if metrics.current.length === 0}
	<EmptyState title="No step runs in this window" description="No steps ran in this time range." />
{:else}
	{@const rows = metrics.current}
	<Section title="Cost by step">
		<BarChart
			title="Cost per step"
			format={formatCost}
			bars={rows.map((r) => ({ label: `${r.phase}/${r.step}`, value: r.costUsd }))}
		/>
	</Section>
	<Section title="Steps" flush>
		<StepMetricsTable {rows} />
	</Section>
{/if}
