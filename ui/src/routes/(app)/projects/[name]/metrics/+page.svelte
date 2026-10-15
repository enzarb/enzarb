<script lang="ts">
	import { page } from '$app/state';
	import BarChart from '#lib/components/BarChart.svelte';
	import ProjectUsageSummary from '#lib/components/ProjectUsageSummary.svelte';
	import TaskCostTable from '#lib/components/TaskCostTable.svelte';
	import EmptyState from '#lib/components/ui/EmptyState.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { getProjectUsage } from '#lib/metrics.remote.ts';
	import { resolveWindow, windowToParams } from '#lib/metrics-window.ts';
	import { formatCost } from '#lib/usage.ts';

	const CHART_TASKS = 30;

	const current = $derived(resolveWindow(page.url.searchParams, new Date()).current);
	const usage = $derived(
		getProjectUsage({ name: page.params.name!, ...Object.fromEntries(windowToParams(current)) })
	);
</script>

{#if usage.error && !usage.current}
	<Notice tone="error" title="Could not load usage"
		>Something went wrong while loading usage.</Notice
	>
{:else if !usage.current}
	<Skeleton label="Loading usage" rows={3} />
{:else if usage.current.total.items === 0}
	<EmptyState
		title="No usage in this window"
		description="No tasks were created in this time range."
	/>
{:else}
	{@const u = usage.current}
	<Section title="Usage and cost">
		<ProjectUsageSummary total={u.total} />
	</Section>
	<Section title="Cost per task">
		<BarChart
			title="Cost per task, oldest first"
			format={formatCost}
			bars={u.tasks
				.slice(-CHART_TASKS)
				.map((t) => ({ label: t.key, value: t.usage?.costUsd ?? null }))}
		/>
		{#if u.tasks.length > CHART_TASKS}
			<p class="ez-muted">
				The chart shows the last {CHART_TASKS} of {u.tasks.length} tasks; the table lists all of them.
			</p>
		{/if}
		<TaskCostTable tasks={u.tasks} />
	</Section>
{/if}
