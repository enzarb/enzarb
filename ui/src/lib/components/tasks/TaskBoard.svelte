<script lang="ts">
	import { placeCards, type Board, type TaskRow } from '#lib/tasks.ts';
	import TaskCard from './TaskCard.svelte';

	/** The project's board from `board_v`: columns in order, cards placed by projection `column`. */
	let {
		board,
		tasks,
		oncommand,
		highlight = null
	}: {
		board: Board;
		tasks: TaskRow[];
		oncommand?: (id: number, command: string) => Promise<unknown>;
		highlight?: number | null;
	} = $props();

	const columns = $derived(placeCards(board, tasks));
</script>

<div class="ez-board" role="group" aria-label="Task board" data-testid="task-board">
	{#each columns as c (c.column.id)}
		{@const over = c.column.limit != null && c.tasks.length > c.column.limit}
		<section class="ez-board__col" aria-label={c.column.title}>
			<h3 class="ez-board__title">
				{c.column.title}
				<span class="ez-muted" class:ez-board__over={over}
					>{c.tasks.length}{c.column.limit != null ? ` / ${c.column.limit}` : ''}</span
				>
			</h3>
			{#each c.tasks as t (t.id)}
				<TaskCard task={t} {oncommand} highlight={t.id === highlight} />
			{:else}
				<p class="ez-muted">No tasks</p>
			{/each}
		</section>
	{:else}
		<p class="ez-muted">This project's board has no columns.</p>
	{/each}
</div>
