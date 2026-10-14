<script lang="ts">
	import ChartRolloutStatus from '#lib/components/ChartRolloutStatus.svelte';
	import ComponentVersionCard from '#lib/components/ComponentVersionCard.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head><title>About · enzarb</title></svelte:head>

<PageHeader title="About" description="Deployed versions and release notes for each component." />

<p>Chart version: <code>{data.chart}</code></p>
<ChartRolloutStatus rollout={data.chartRollout} />

{#each data.components as component (component.name)}
	<ComponentVersionCard {component} />
{/each}
