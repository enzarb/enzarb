<script lang="ts">
	import AppHeader from '#lib/components/AppHeader.svelte';
	import Nav from '#lib/components/Nav.svelte';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();
</script>

<div class="shell">
	<AppHeader />
	<Nav />
	<main class="content">{@render children()}</main>
</div>
