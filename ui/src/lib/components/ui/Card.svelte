<script lang="ts">
	import type { Snippet } from 'svelte';

	/**
	 * Surface with a border and soft elevation. `flush` removes the padding (for tables and lists
	 * that run edge to edge), `sunken` drops the elevation (a quieter nested surface).
	 */
	let {
		flush = false,
		sunken = false,
		children,
		...rest
	}: { flush?: boolean; sunken?: boolean; children: Snippet; 'aria-label'?: string } = $props();
</script>

<div {...rest} class="ez-card" class:ez-card--flush={flush} class:ez-card--sunken={sunken}>
	{@render children()}
</div>
