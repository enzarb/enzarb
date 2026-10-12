<script lang="ts">
	import { page } from '$app/state';
	import Chat from '#lib/components/chat/Chat.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import { getProjectContext } from '#lib/project-context.svelte.ts';

	const ctx = getProjectContext();
</script>

<svelte:head><title>Chat · {page.params.name} · enzarb</title></svelte:head>

{#if ctx.project.pendingChange}
	<!-- Drain-first: the sandbox closes open chats with `shutdown` before it restarts. -->
	<Notice tone="info">This chat will reconnect after the pending sandbox change is applied.</Notice>
{/if}
{#if ctx.project.ready}
	<div class="ez-project-chat">
		{#key ctx.project.name}
			<Chat project={ctx.project.name} />
		{/key}
	</div>
{:else}
	<Notice tone="info">Sandbox is still booting. Chat is available once it is ready.</Notice>
{/if}
