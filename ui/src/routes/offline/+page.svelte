<script lang="ts">
	import EmptyState from '#lib/components/ui/EmptyState.svelte';
</script>

<svelte:head>
	<title>Offline · enzarb</title>
</svelte:head>

<main class="ez-content">
	<EmptyState
		title="You are offline"
		description="enzarb needs a connection. Try again once you are back online."
	/>
</main>
