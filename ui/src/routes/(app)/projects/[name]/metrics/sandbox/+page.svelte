<script lang="ts">
	import { page } from '$app/state';
	import SandboxUtilization from '#lib/components/SandboxUtilization.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { getSandboxStats } from '#lib/metrics.remote.ts';
	import { resolveWindow, windowToParams } from '#lib/metrics-window.ts';

	const current = $derived(resolveWindow(page.url.searchParams, new Date()).current);
	const sandbox = $derived(
		getSandboxStats({ name: page.params.name!, ...Object.fromEntries(windowToParams(current)) })
	);
</script>

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
