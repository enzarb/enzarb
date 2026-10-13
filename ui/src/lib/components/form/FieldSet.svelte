<script lang="ts">
	import type { Snippet } from 'svelte';

	/** A titled group of related controls (e.g. a set of checkboxes); the legend names the group. */
	let { legend, children }: { legend: string; children: Snippet } = $props();
</script>

<fieldset class="ez-fieldset">
	<legend class="ez-fieldset__legend">{legend}</legend>
	<div class="ez-fieldset__body">{@render children()}</div>
</fieldset>
