<script lang="ts">
	import { page } from '$app/state';
	import AppHeader from '#lib/components/AppHeader.svelte';
	import InstallBanner from '#lib/components/InstallBanner.svelte';
	import Nav from '#lib/components/Nav.svelte';
	import { watchWorkItems } from '#lib/live.ts';
	import { NeedsYouStore, setNeedsYou } from '#lib/needs-you.svelte.ts';
	import { getWorkItem, listNeedsYou, submitAction } from '#lib/work.remote.ts';
	import type { LayoutProps } from './$types';

	let { children, data }: LayoutProps = $props();

	// One store and one live subscription for the whole app: the nav badge and /work share it.
	const needsYou = new NeedsYouStore({
		list: async () => {
			const q = listNeedsYou();
			await q.refresh();
			return q.current ?? [];
		},
		get: async (id) => {
			const q = getWorkItem(id);
			await q.refresh();
			return q.current!;
		},
		submit: (id, action) => submitAction({ id, action }),
		watch: watchWorkItems
	});
	setNeedsYou(needsYou);
	$effect(() => needsYou.start());

	// Refresh re-runs the GitHub authorize flow and comes back to this page.
	const refreshHref = $derived(
		`/login/github?refresh=1&return=${encodeURIComponent(page.url.pathname + page.url.search)}`
	);
</script>

<div class="ez-shell">
	<AppHeader />
	<Nav needsYou={needsYou.count} />
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
