<script lang="ts">
	import { page } from '$app/state';
	import StepTimingTable from '#lib/components/StepTimingTable.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { getProjectStepTiming } from '#lib/metrics.remote.ts';
	import { resolveWindow, windowToParams } from '#lib/metrics-window.ts';

	const current = $derived(resolveWindow(page.url.searchParams, new Date()).current);
	const timing = $derived(
		getProjectStepTiming({
			name: page.params.name!,
			...Object.fromEntries(windowToParams(current))
		})
	);
</script>

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
