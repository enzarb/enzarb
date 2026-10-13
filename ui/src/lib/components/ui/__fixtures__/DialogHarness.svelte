<script lang="ts">
	// Test-only: a dialog with a trigger, two inner buttons and an outside button.
	import Dialog from '../Dialog.svelte';
	let { confirmed = $bindable(0) }: { confirmed?: number } = $props();
	let open = $state(false);
</script>

<button onclick={() => (open = true)}>Open</button>
<Dialog bind:open title="Edit thing" description="Make a change.">
	<input aria-label="Name" />
	{#snippet footer()}
		<button onclick={() => (open = false)}>Close me</button>
		<button onclick={() => confirmed++}>Save</button>
	{/snippet}
</Dialog>
<button>Outside</button>
