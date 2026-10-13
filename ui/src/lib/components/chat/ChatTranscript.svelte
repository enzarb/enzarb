<script lang="ts">
	import type { ChatItem, PermissionChoice } from '#lib/acp/types.ts';
	import ChatMessage from './ChatMessage.svelte';
	import PermissionRequest from './PermissionRequest.svelte';
	import ToolCall from './ToolCall.svelte';

	/** The conversation: a polite live region so new messages are announced. */
	let {
		items,
		epoch,
		onAnswer
	}: {
		items: ChatItem[];
		epoch: number;
		onAnswer: (id: string, choice: PermissionChoice, epoch: number) => void;
	} = $props();
</script>

<div class="ez-chat-log" role="log" aria-live="polite" aria-label="Conversation">
	{#each items as item (item.id)}
		{#if item.kind === 'user' || item.kind === 'agent' || item.kind === 'thought'}
			<ChatMessage kind={item.kind} text={item.text} />
		{:else if item.kind === 'tool'}
			<ToolCall tool={item} />
		{:else if item.kind === 'plan'}
			<div class="ez-chat-plan">
				<p class="ez-chat-msg__who">Plan</p>
				<ol>
					{#each item.entries as entry, i (i)}
						<li data-status={entry.status}>
							{entry.content} <span class="ez-chat-plan__state">({entry.status})</span>
						</li>
					{/each}
				</ol>
			</div>
		{:else if item.kind === 'permission'}
			<PermissionRequest
				id={item.id}
				toolCall={item.toolCall}
				options={item.options}
				status={item.status}
				hint={item.hint}
				reason={item.reason}
				{epoch}
				{onAnswer}
			/>
		{:else if item.kind === 'notice'}
			<div class="ez-chat-notice" role="note">{item.text}</div>
		{/if}
	{/each}
</div>
