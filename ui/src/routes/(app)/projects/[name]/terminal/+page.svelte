<script lang="ts">
	import { page } from '$app/state';
	import Notice from '#lib/components/ui/Notice.svelte';
	import Terminal from '#lib/components/Terminal.svelte';
	import { getProjectContext } from '#lib/project-context.svelte.ts';

	const ctx = getProjectContext();
</script>

<svelte:head><title>Terminal · {page.params.name} · enzarb</title></svelte:head>

{#if ctx.project.ready}
	<div class="ez-project-terminal">
		{#key ctx.project.name}
			<Terminal project={ctx.project.name} fill />
		{/key}
	</div>
{:else}
	<Notice tone="info">Sandbox is still booting. The terminal is available once it is ready.</Notice>
{/if}
