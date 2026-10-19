<script lang="ts">
	import { childProgress, linkDone, type TaskLink } from '#lib/tasks.ts';
	import TaskStatus from './TaskStatus.svelte';

	/**
	 * A task's place in an epic (`task_v.parent` / `task_v.children`): the epic it belongs to, or
	 * its sub-issues with how many are done. `compact` is the one-line form for a board card.
	 */
	let {
		parent,
		subIssues,
		compact = false
	}: { parent: TaskLink | null; subIssues: TaskLink[]; compact?: boolean } = $props();

	const progress = $derived(childProgress(subIssues));
	const label = (l: TaskLink) =>
		l.title || (l.key ?? '').replace(/^[a-z]+:/, '') || (l.taskId ? `Task #${l.taskId}` : '');
</script>

{#if compact}
	{#if parent?.taskId}
		<p class="ez-muted ez-task-family" data-testid="task-parent">
			Part of <a href="/task/{parent.taskId}">{label(parent)}</a>
		</p>
	{/if}
	{#if subIssues.length}
		<p class="ez-muted ez-task-family" data-testid="task-progress">
			Sub-issues {progress[0]}/{progress[1]} done
		</p>
	{/if}
{:else}
	{#if parent?.taskId}
		<p data-testid="task-parent">
			Part of <a href="/task/{parent.taskId}">{label(parent)}</a>
			{#if parent.status}<TaskStatus status={parent.status} endStatus={parent.endStatus} />{/if}
		</p>
	{/if}
	{#if subIssues.length}
		<p class="ez-muted" data-testid="task-progress">
			{progress[0]} of {progress[1]} sub-issues done
		</p>
		<ul class="ez-task-family__list" data-testid="task-children">
			{#each subIssues as c (c.key ?? c.taskId)}
				<li class:ez-task-family__done={linkDone(c)}>
					{#if c.taskId}
						<a href="/task/{c.taskId}">{label(c)}</a>
						{#if c.status}<TaskStatus status={c.status} endStatus={c.endStatus} />{/if}
					{:else}
						<code>{label(c)}</code> <span class="ez-muted">no task yet</span>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
{/if}
