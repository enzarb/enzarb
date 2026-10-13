<script lang="ts">
	import { page } from '$app/state';
	import AppHeader from '#lib/components/AppHeader.svelte';
	import InstallBanner from '#lib/components/InstallBanner.svelte';
	import Nav from '#lib/components/Nav.svelte';
	import type { LayoutProps } from './$types';

	let { children, data }: LayoutProps = $props();

	// Refresh re-runs the GitHub authorize flow and comes back to this page.
	const refreshHref = $derived(
		`/login/github?refresh=1&return=${encodeURIComponent(page.url.pathname + page.url.search)}`
	);
</script>

<div class="ez-shell">
	<AppHeader />
	<Nav />
	<main class="ez-content">
		<div class="ez-page">
			{#if data.installBanner}
				<InstallBanner installUrl={data.installBanner.installUrl} {refreshHref} />
			{/if}
			{@render children()}
		</div>
	</main>
	<footer class="ez-footer">
		<a href="/about">About</a>
	</footer>
</div>
