<script lang="ts">
	import { messages, type Issues } from './util.ts';

	let { id, help, issues }: { id: string; help?: string; issues?: Issues } = $props();
	const errors = $derived(messages(issues));
</script>

{#if help}<p class="field-help muted" id="{id}-help">{help}</p>{/if}
{#if errors.length}
	<div id="{id}-error">
		{#each errors as msg, n (n)}<p class="field-error">{msg}</p>{/each}
	</div>
{/if}
