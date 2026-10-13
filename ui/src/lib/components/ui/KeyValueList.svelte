<script lang="ts" module>
	import type { Snippet } from 'svelte';
	import type { Tone } from './status.ts';

	export interface KeyValue {
		term: string;
		/** Plain text value; use `children` for rich values. */
		value?: string;
		/** Render `value` as a Badge in this tone (on/off flags, states). */
		tone?: Tone;
		children?: Snippet;
	}
</script>

<script lang="ts">
	import Badge from './Badge.svelte';

	/** Definition list of labelled facts (identifiers, settings). Two columns, one on phones. */
	let { items }: { items: KeyValue[] } = $props();
</script>

<dl class="ez-kv">
	{#each items as item (item.term)}
		<dt class="ez-kv__term">{item.term}</dt>
		<dd class="ez-kv__value">
			{#if item.children}{@render item.children()}{:else if item.tone}<Badge
					tone={item.tone}
					hollow={item.tone === 'neutral'}>{item.value}</Badge
				>{:else}{item.value}{/if}
		</dd>
	{/each}
</dl>
