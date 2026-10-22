<script lang="ts">
	import { invalidateAll } from '$app/navigation';
	import { refreshRepositoryChoices } from '#lib/projects.remote.ts';
	import AppHeader from '#lib/components/AppHeader.svelte';
	import InstallBanner from '#lib/components/InstallBanner.svelte';
	import Nav from '#lib/components/Nav.svelte';
	import type { LayoutProps } from './$types';

	let { children, data }: LayoutProps = $props();
</script>

<div class="ez-shell">
	<AppHeader />
	<Nav />
	<main class="ez-content">
		<div class="ez-page">
			{#await data.installBanner then banner}
				{#if banner}
					<InstallBanner
						installUrl={banner.installUrl}
						refresh={async () => {
							await refreshRepositoryChoices();
							await invalidateAll();
						}}
					/>
				{/if}
			{/await}
			{@render children()}
		</div>
	</main>
	<footer class="ez-footer">
		<a href="/about">About</a>
	</footer>
</div>
