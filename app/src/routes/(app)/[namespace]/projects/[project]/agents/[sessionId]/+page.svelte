<script lang="ts">
	import { getProject } from '$lib/remote/projects.remote';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import AgentPane from '$lib/agent/AgentPane.svelte';

	const sessionId = $derived(page.params.sessionId!);
	const backHref = $derived(`/${page.params.namespace}/projects/${page.params.project}/agents`);

	let agentBase = $state('');

	onMount(async () => {
		try {
			const project = await getProject();
			const path = project?.status?.agentPath;
			if (path) agentBase = `https://enzarb.dev${path}`;
		} catch {}
	});
</script>

<div class="chat-page">
	<div class="chat-header">
		<a href={backHref} class="back">← Sessions</a>
	</div>
	<div class="pane-wrap">
		{#if agentBase}
			<AgentPane {agentBase} namespace={page.params.namespace!} project={page.params.project!} {sessionId} />
		{/if}
	</div>
</div>

<style>
	.chat-page { display: flex; flex-direction: column; height: 100%; overflow: hidden; }
	.chat-header { display: flex; align-items: center; gap: 0.75rem; padding: 0.5rem 0; border-bottom: 1px solid var(--color-border); }
	.back { font-size: 12px; color: var(--color-text-muted); text-decoration: none; }
	.back:hover { color: var(--color-text); }
	.pane-wrap { flex: 1; min-height: 0; }
</style>
