<script lang="ts">
	import Tooltip from './ui/Tooltip.svelte';

	/**
	 * A compact figure (`12.3k`, `$0.42`) with the exact breakdown in a tooltip. Without a hint it
	 * is plain text, so an unknown value (a dash) is not a useless tab stop.
	 */
	let { value, hint }: { value: string; hint?: string } = $props();
</script>

{#if hint}
	<Tooltip text={hint}>
		{#snippet trigger(props)}
			<button type="button" class="ez-usage" {...props}>{value}</button>
		{/snippet}
	</Tooltip>
{:else}
	<span class:ez-subtle={value === '—'}>{value}</span>
{/if}
