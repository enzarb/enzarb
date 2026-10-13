<script lang="ts">
	import type { Component } from 'svelte';

	// Renders the project layout around a tab page, like SvelteKit does.
	let {
		Layout,
		Child
	}: { Layout: Component<{ children: import('svelte').Snippet }>; Child: Component } = $props();
</script>

<Layout><Child /></Layout>
