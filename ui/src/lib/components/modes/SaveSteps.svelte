<script lang="ts">
	import StepList, { type StepItem } from '../ui/StepList.svelte';

	/** Progress of a save: validate, then commit. Shown only once a save has started. */
	let { items }: { items: StepItem[] } = $props();
</script>

{#if items.length}<StepList label="Saving the modes" {items} />{/if}
