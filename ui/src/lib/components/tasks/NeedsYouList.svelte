<script lang="ts">
	import EmptyState from '#lib/components/ui/EmptyState.svelte';
	import { groupByProject, type TaskRow } from '#lib/tasks.ts';
	import TaskCard from './TaskCard.svelte';

	/** "Waiting on you": projection rows with `needs_you`, grouped by project. */
	let {
		tasks,
		oncommand,
		highlight = null
	}: {
		tasks: TaskRow[];
		oncommand: (id: number, command: string) => Promise<unknown>;
		highlight?: number | null;
	} = $props();

	const groups = $derived(groupByProject(tasks));
</script>

{#each groups as [project, rows] (project)}
	<section class="ez-needs-you__group" aria-label="Project {project}">
		<h2 class="ez-section__title"><a href="/projects/{project}/tasks">{project}</a></h2>
		<div class="ez-needs-you__cards">
			{#each rows as t (t.id)}
				<TaskCard task={t} {oncommand} highlight={t.id === highlight} />
			{/each}
		</div>
	</section>
{:else}
	<EmptyState title="Nothing is waiting on you" description="Tasks that need you show up here." />
{/each}
