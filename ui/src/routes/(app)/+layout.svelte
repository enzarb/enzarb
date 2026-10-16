<script lang="ts">
	import { invalidateAll } from '$app/navigation';
	import { refreshRepositoryChoices } from '#lib/projects.remote.ts';
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
			return q.current ?? { items: [], total: 0 };
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
</script>

<div class="ez-shell">
	<AppHeader />
	<Nav needsYou={needsYou.count} needsYouMore={needsYou.overflow > 0} />
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
