<script lang="ts">
	import { page } from '$app/state';
	import RunList from '#lib/components/RunList.svelte';
	import { listRuns } from '#lib/runs.remote.ts';
	import { isEnded } from '#lib/runs.ts';

	const name = $derived(page.params.name!);
	const runs = $derived(listRuns({ name }));

	// Poll while a run is live (no timers when everything has ended).
	$effect(() => {
		if (!(runs.current ?? []).some((r) => !isEnded(r))) return;
		const id = setInterval(() => void runs.refresh().catch(() => {}), 3000);
		return () => clearInterval(id);
	});
</script>

<svelte:head><title>Runs · {page.params.name} · enzarb</title></svelte:head>

<RunList
	runs={runs.current}
	base="/projects/{encodeURIComponent(name)}/runs"
	failed={!!runs.error && !runs.current}
	retry={() => void runs.refresh()}
/>
