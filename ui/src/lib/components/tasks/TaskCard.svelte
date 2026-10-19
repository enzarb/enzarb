<script lang="ts">
	import type { TaskRow } from '#lib/tasks.ts';
	import { formatCost } from '#lib/usage.ts';
	import TaskFamily from './TaskFamily.svelte';
	import SubjectLinks from './SubjectLinks.svelte';
	import TaskActions from './TaskActions.svelte';
	import TaskStatus from './TaskStatus.svelte';

	/**
	 * One task as the projection describes it: title, the wait's reason and text, its declared
	 * buttons, artifact links and subjects. Nothing here is derived from workflow state.
	 */
	let {
		task,
		oncommand,
		highlight = false
	}: {
		task: TaskRow;
		oncommand?: (id: number, command: string) => Promise<unknown>;
		highlight?: boolean;
	} = $props();
</script>

<article
	class={['ez-card', task.needsYou && 'ez-card--needs', highlight && 'ez-card--highlight']}
	data-testid="task-card"
	aria-labelledby="task-{task.id}-title"
>
	<header class="ez-task-card__head">
		<a id="task-{task.id}-title" class="ez-card__title" href="/task/{task.id}">{task.title}</a>
		<TaskStatus status={task.status} endStatus={task.endStatus} />
	</header>
	{#if task.reason}<p class="ez-card__reason">{task.reason}</p>{/if}
	{#if task.text}<p class="ez-card__wait">{task.text}</p>{/if}
	{#if oncommand && task.actions.length}
		<TaskActions actions={task.actions} oncommand={(c) => oncommand(task.id, c)} />
	{/if}
	{#if task.artifacts.length}
		<ul class="ez-task-card__artifacts" aria-label="Artifacts">
			{#each task.artifacts as a, i (i)}
				<li><a href="/task/{task.id}#artifact-{i}">{a.title}</a></li>
			{/each}
		</ul>
	{/if}
	<TaskFamily parent={task.parent} subIssues={task.children} compact />
	<SubjectLinks subjects={task.subjects} />
	{#if task.costUsd !== null}<p class="ez-muted ez-card__usage">
			{formatCost(task.costUsd)}
		</p>{/if}
</article>
