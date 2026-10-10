<script lang="ts">
	import ProjectBootCard from './ProjectBootCard.svelte';
	import LinkButton from './ui/LinkButton.svelte';
	import Notice from './ui/Notice.svelte';
	import Section from './ui/Section.svelte';
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

<div class="ez-stack">
	<Notice tone={failed.length ? 'warning' : 'success'}>
		{created.length} created{failed.length ? `, ${failed.length} failed` : ''}.
	</Notice>

	{#if failed.length}
		<Section title="Not created">
			<ul>
				{#each failed as r, i (i)}
					<li><code>{r.name}</code>: {r.status === 'failed' ? r.reason : ''}</li>
				{/each}
			</ul>
		</Section>
	{/if}

	{#each created as r (r.name)}
		<ProjectBootCard name={r.name} project={views[r.name]} />
	{/each}

	<div class="ez-cluster">
		<LinkButton href="/projects">All projects</LinkButton>
		<LinkButton href="/projects/new" data-sveltekit-reload>Create more</LinkButton>
	</div>
</div>
