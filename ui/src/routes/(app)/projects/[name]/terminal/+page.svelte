<script lang="ts">
	import { page } from '$app/state';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import Terminal from '#lib/components/Terminal.svelte';
</script>

<svelte:head><title>Terminal · {page.params.name} · enzarb</title></svelte:head>
<PageHeader title="Terminal">
	{#snippet meta()}
		<span class="ez-muted">
			Shell in the sandbox for <a href="/projects/{page.params.name}"
				><code>{page.params.name}</code></a
			>.
		</span>
	{/snippet}
</PageHeader>
{#key page.params.name}
	<Terminal project={page.params.name!} />
{/key}
