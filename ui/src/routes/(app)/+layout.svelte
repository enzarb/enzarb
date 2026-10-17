<script lang="ts">
	import { invalidateAll } from '$app/navigation';
	import { refreshRepositoryChoices } from '#lib/projects.remote.ts';
	import AppHeader from '#lib/components/AppHeader.svelte';
	import InstallBanner from '#lib/components/InstallBanner.svelte';
	import Nav from '#lib/components/Nav.svelte';
	import { watchTasks } from '#lib/live.ts';
	import { NeedsYouStore, setNeedsYou } from '#lib/needs-you.svelte.ts';
	import { getTask, listNeedsYou } from '#lib/tasks.remote.ts';
	import type { LayoutProps } from './$types';

	let { children, data }: LayoutProps = $props();

	// One store and one live subscription for the whole app: the nav badge and /tasks share it.
	const needsYou = new NeedsYouStore({
		list: async () => {
			const q = listNeedsYou();
			await q.refresh();
			return q.current ?? [];
		},
		get: async (id) => {
			const q = getTask(id);
			await q.refresh();
			return q.current ?? null;
		},
		watch: watchTasks
	});
	setNeedsYou(needsYou);
	$effect(() => needsYou.start());
</script>

<div class="ez-shell">
	<AppHeader />
	<Nav needsYou={needsYou.count} />
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
