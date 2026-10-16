<script lang="ts">
	import Button from '#lib/components/form/Button.svelte';
	import type { ChatRow, SessionEvent } from '#lib/tasks.ts';
	import SessionInspector from './SessionInspector.svelte';

	/** The user's chats in this project (`chat_v`), each readable in the session inspector. */
	let {
		chats,
		load
	}: {
		chats: ChatRow[];
		load: (runId: string, afterSeq: number) => Promise<SessionEvent[]>;
	} = $props();

	let open = $state<string | null>(null);
</script>

{#each chats as c (c.runId)}
	<div class="ez-step-run">
		<div class="ez-step-run__head">
			<strong>{c.title ?? c.runId}</strong>
			<span class="ez-muted"
				>{new Date(c.startedAt).toLocaleString()} · {c.events} event{c.events === 1
					? ''
					: 's'}</span
			>
		</div>
		<Button
			size="sm"
			variant="ghost"
			aria-expanded={open === c.runId}
			onclick={() => (open = open === c.runId ? null : c.runId)}
			>{open === c.runId ? 'Hide history' : 'Show history'}</Button
		>
		{#if open === c.runId}
			<SessionInspector load={(after) => load(c.runId, after)} />
		{/if}
	</div>
{:else}
	<p class="ez-muted">No previous chats.</p>
{/each}
