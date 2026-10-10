<script lang="ts">
	import ProjectBootCard from './ProjectBootCard.svelte';
	import { isTerminalFailure } from '#lib/conditions.ts';
	import type { PlanResult } from '#lib/onboarding.ts';
	import type { ProjectView } from '#lib/project-types.ts';

	let {
		results,
		load,
		intervalMs = 2000
	}: {
		results: PlanResult[];
		/** One owner-scoped listing of the user's Projects. */
		load: () => Promise<ProjectView[]>;
		intervalMs?: number;
	} = $props();

	const created = $derived(results.filter((r) => r.status === 'created'));
	const failed = $derived(results.filter((r) => r.status === 'failed'));

	let views = $state<Record<string, ProjectView>>({});

	const settled = (p: ProjectView | undefined) =>
		!!p && (p.ready || isTerminalFailure(p.conditions));
	const allSettled = $derived(created.every((r) => settled(views[r.name])));

	// One poll feeds every card; it stops once each Project is Ready or has failed.
	$effect(() => {
		if (allSettled) return;
		let stopped = false;
		const tick = async () => {
			try {
				const list = await load();
				if (stopped) return;
				views = Object.fromEntries(list.map((p) => [p.name, p]));
			} catch {
				// keep the last known state and try again
			}
		};
		void tick();
		const id = setInterval(() => void tick(), intervalMs);
		return () => {
			stopped = true;
			clearInterval(id);
		};
	});
</script>

<p role="status">
	{created.length} created{failed.length ? `, ${failed.length} failed` : ''}.
</p>

{#if failed.length}
	<section class="card stack" aria-label="Failed projects">
		<h2>Not created</h2>
		<ul>
			{#each failed as r, i (i)}
				<li><code>{r.name}</code>: {r.status === 'failed' ? r.reason : ''}</li>
			{/each}
		</ul>
	</section>
{/if}

{#each created as r (r.name)}
	<ProjectBootCard name={r.name} project={views[r.name]} />
{/each}

<p>
	<a href="/projects">All projects</a> ·
	<a href="/projects/new" data-sveltekit-reload>Create more</a>
</p>
