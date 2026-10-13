<script lang="ts">
	import { page } from '$app/state';
	import Chat from '#lib/components/chat/Chat.svelte';
	import ClaudeTokenNotice from '#lib/components/ClaudeTokenNotice.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { claudeCredential } from '#lib/claude.remote.ts';
	import Notice from '#lib/components/ui/Notice.svelte';
	import { getProjectContext } from '#lib/project-context.svelte.ts';

	const ctx = getProjectContext();
	// Presence only. A failed read falls through to the chat: the proxy answers `no_credential` then.
	const credential = claudeCredential();
</script>

<svelte:head><title>Chat · {page.params.name} · enzarb</title></svelte:head>

{#if ctx.project.pendingChange}
	<!-- Drain-first: the sandbox closes open chats with `shutdown` before it restarts. -->
	<Notice tone="info">This chat will reconnect after the pending sandbox change is applied.</Notice>
{/if}
{#if credential.current && !credential.current.configured}
	<ClaudeTokenNotice />
{:else if !credential.current && !credential.error}
	<Skeleton label="Checking your Claude token" rows={1} />
{:else if ctx.project.ready}
	<div class="ez-project-chat">
		{#key ctx.project.name}
			<Chat project={ctx.project.name} />
		{/key}
	</div>
{:else}
	<Notice tone="info">Sandbox is still booting. Chat is available once it is ready.</Notice>
{/if}
