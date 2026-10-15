<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import BarChart from '#lib/components/BarChart.svelte';
	import ProjectUsageSummary from '#lib/components/ProjectUsageSummary.svelte';
	import SandboxUtilization from '#lib/components/SandboxUtilization.svelte';
	import StepTimingTable from '#lib/components/StepTimingTable.svelte';
	import TaskCostTable from '#lib/components/TaskCostTable.svelte';
	import WindowPicker from '#lib/components/WindowPicker.svelte';
	import EmptyState from '#lib/components/ui/EmptyState.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { getProjectStepTiming, getProjectUsage, getSandboxStats } from '#lib/metrics.remote.ts';
	import { defaultWindow, parseWindow, windowToParams } from '#lib/metrics-window.ts';
	import { formatCost } from '#lib/usage.ts';

	const CHART_TASKS = 30;

	// Presets are anchored to the moment the URL last changed, so a reload or a shared link is stable.
	const parsed = $derived(parseWindow(page.url.searchParams, new Date()));
	const current = $derived(parsed.ok ? parsed.window : defaultWindow(new Date()));
	const usage = $derived(
		getProjectUsage({ name: page.params.name!, ...Object.fromEntries(windowToParams(current)) })
	);

	const timing = $derived(
		getProjectStepTiming({
			name: page.params.name!,
			...Object.fromEntries(windowToParams(current))
		})
	);

	const sandbox = $derived(
		getSandboxStats({ name: page.params.name!, ...Object.fromEntries(windowToParams(current)) })
	);

	function change(params: URLSearchParams) {
		void goto(`?${params}`, { replace: true, reset: false });
	}
</script>

<svelte:head><title>Metrics · {page.params.name} · enzarb</title></svelte:head>

<div class="ez-stack">
	{#if !parsed.ok}
		<Notice tone="warning" title="Showing the last 7 days">{parsed.error.message}</Notice>
	{/if}
	<WindowPicker value={current} onchange={change} />

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

	<Section title="Step timing">
		{#if timing.error && !timing.current}
			<Notice tone="error" title="Could not load step timing"
				>Something went wrong while loading step timing.</Notice
			>
		{:else if !timing.current}
			<Skeleton label="Loading step timing" rows={3} />
		{:else}
			<StepTimingTable timing={timing.current} />
		{/if}
	</Section>

	<Section title="Sandbox utilization">
		{#if sandbox.error && !sandbox.current}
			<Notice tone="error" title="Could not load sandbox utilization"
				>Something went wrong while loading sandbox utilization.</Notice
			>
		{:else if !sandbox.current}
			<Skeleton label="Loading sandbox utilization" rows={3} />
		{:else}
			<SandboxUtilization stats={sandbox.current} />
		{/if}
	</Section>
</div>
