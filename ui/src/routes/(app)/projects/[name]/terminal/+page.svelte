<script lang="ts">
	import { page } from '$app/state';
	import PageHeading from '#lib/components/PageHeading.svelte';
	import Terminal from '#lib/components/Terminal.svelte';
</script>

<svelte:head><title>Terminal · {page.params.name} · enzarb</title></svelte:head>

<PageHeading title="Terminal" />
<p class="muted">
	Shell in the sandbox for <a href="/projects/{page.params.name}"><code>{page.params.name}</code></a
	>.
</p>
{#key page.params.name}
	<Terminal project={page.params.name!} />
{/key}
