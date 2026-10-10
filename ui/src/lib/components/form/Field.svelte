<script lang="ts">
	import type { Snippet } from 'svelte';
	import FieldMessages from './FieldMessages.svelte';
	import type { Issues } from './util.ts';

	/** Label + control + help + errors. The control must use `id` and labelledby `${id}-label`. */
	let {
		id,
		label,
		help,
		issues,
		children
	}: { id: string; label: string; help?: string; issues?: Issues; children: Snippet } = $props();
</script>

<div class="ez-field">
	<label class="ez-field__label" id="{id}-label" for={id}>{label}</label>
	{@render children()}
	<FieldMessages {id} {help} {issues} />
</div>
