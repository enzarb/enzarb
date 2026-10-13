<script lang="ts">
	import type { ToolView } from '#lib/acp/types.ts';

	/** A tool call the agent ran, with its latest status and output. */
	let { tool }: { tool: ToolView } = $props();
</script>

<div class="ez-chat-tool" data-status={tool.status ?? 'pending'}>
	<div class="ez-chat-tool__head">
		<span class="ez-chat-tool__title">{tool.title || 'Tool call'}</span>
		<span class="ez-chat-tool__status">{tool.status ?? 'pending'}</span>
	</div>
	{#if tool.text}<pre class="ez-chat-tool__out">{tool.text}</pre>{/if}
</div>
