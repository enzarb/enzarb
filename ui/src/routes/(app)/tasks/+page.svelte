<script lang="ts">
	import { page } from '$app/state';
	import NeedsYouList from '#lib/components/tasks/NeedsYouList.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { getNeedsYou } from '#lib/needs-you.svelte.ts';
	import { submitTask } from '#lib/tasks.remote.ts';

	const needsYou = getNeedsYou();
	const highlight = $derived(Number(page.url.searchParams.get('highlight')) || null);
</script>

<svelte:head><title>Tasks · enzarb</title></svelte:head>
<PageHeader
	title="Waiting on you"
	description="Tasks across your projects that wait on you. Each project's full board is on its Tasks tab."
/>

{#if needsYou.failed && !needsYou.loaded}
	<Notice tone="error">Could not load the tasks waiting on you.</Notice>
{:else if !needsYou.loaded}
	<Skeleton label="Loading tasks" rows={3} />
{:else}
	{#if needsYou.failed}<Notice tone="warning">Showing the last loaded list.</Notice>{/if}
	<NeedsYouList
		tasks={needsYou.items}
		{highlight}
		oncommand={(id, command) => submitTask({ id, kind: 'command', data: { command } })}
	/>
{/if}
