<script lang="ts">
	import FindingList from '#lib/components/findings/FindingList.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { getNeedsYou } from '#lib/needs-you.svelte.ts';
	import { listFindings } from '#lib/tasks.remote.ts';

	/** Findings are recomputed about every 60 s and raise no change event, so the page also polls. */
	const POLL_MS = 30_000;

	const findings = listFindings(undefined);
	const needsYou = getNeedsYou();
	let now = $state(Date.now());

	$effect(() => {
		const refresh = () => {
			now = Date.now();
			void findings.refresh();
		};
		const stop = needsYou.onChange(refresh);
		const timer = setInterval(refresh, POLL_MS);
		return () => {
			stop();
			clearInterval(timer);
		};
	});
</script>

<svelte:head><title>Platform · enzarb</title></svelte:head>
<PageHeader
	title="Platform"
	description="Open liveness findings across your projects. They are observations only and clear when the condition does."
/>

<svelte:boundary>
	{#if findings.current}
		{#if findings.error}<Notice tone="warning">Showing the last loaded findings.</Notice>{/if}
		<FindingList findings={findings.current} {now} />
	{:else}
		<Skeleton label="Loading findings" rows={3} />
	{/if}
	{#snippet failed()}
		<Notice tone="error">Could not load the findings.</Notice>
	{/snippet}
</svelte:boundary>
