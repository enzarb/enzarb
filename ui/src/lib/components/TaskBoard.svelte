<script lang="ts">
	import {
		BOARD_COLUMNS,
		DONE_LIMIT,
		groupByColumn,
		type WorkAction,
		type WorkItemSummary
	} from '#lib/work-items.ts';
	import TaskCard from './TaskCard.svelte';
	import Button from './form/Button.svelte';
	import Badge from './ui/Badge.svelte';
	import EmptyState from './ui/EmptyState.svelte';
	import Skeleton from './ui/Skeleton.svelte';

	/**
	 * Read-only Kanban board of tasks. Columns follow the process, left to right (`BOARD_COLUMNS`);
	 * the ones waiting on the user are accented and counted. A card sits in the column its state
	 * decides, so it moves by itself when a live update changes the item. Nothing is dragged:
	 * the workflow engine drives the transitions, the card offers the actions.
	 */
	let {
		items,
		project,
		now = Date.now(),
		loading = false,
		onaction,
		onplan,
		onreview
	}: {
		/** Newest first; the board keeps that order inside each column. */
		items: WorkItemSummary[];
		project?: string;
		now?: number;
		loading?: boolean;
		onaction: (id: number, action: WorkAction) => Promise<void>;
		onplan?: (item: WorkItemSummary) => void;
		onreview?: (item: WorkItemSummary) => void;
	} = $props();

	const columns = $derived(groupByColumn(items));
	let showAllDone = $state(false);

	let pending = $state<{ id: number; action: WorkAction } | null>(null);
	let errors = $state<Record<number, string>>({});

	async function act(id: number, action: WorkAction) {
		pending = { id, action };
		errors[id] = '';
		try {
			await onaction(id, action);
		} catch {
			errors[id] = `Could not ${action} this item. Its state may have changed.`;
		} finally {
			pending = null;
		}
	}

	const shown = (id: string, list: WorkItemSummary[]) =>
		id === 'done' && !showAllDone ? list.slice(0, DONE_LIMIT) : list;
</script>

{#if loading}
	<Skeleton label="Loading tasks" rows={4} />
{:else if items.length === 0}
	<EmptyState
		title="No tasks yet"
		description="Tasks appear here when an issue, change request or mention reaches a project."
	/>
{:else}
	<div class="ez-board" role="group" aria-label="Task board">
		{#each BOARD_COLUMNS as col (col.id)}
			{@const all = columns[col.id]}
			<section
				class="ez-board__col"
				class:ez-board__col--needs={col.needsYou}
				aria-labelledby="ez-board-{col.id}"
			>
				<h3 class="ez-board__title" id="ez-board-{col.id}">
					{col.title}
					<Badge tone={col.needsYou && all.length ? 'accent' : 'neutral'}>{all.length}</Badge>
				</h3>
				{#if all.length}
					<ul class="ez-board__cards">
						{#each shown(col.id, all) as item (item.id)}
							<TaskCard
								{item}
								{project}
								{now}
								pending={pending?.id === item.id ? pending.action : null}
								error={errors[item.id] ?? ''}
								onaction={(a) => act(item.id, a)}
								onplan={onplan ? () => onplan(item) : undefined}
								onreview={onreview ? () => onreview(item) : undefined}
							/>
						{/each}
					</ul>
					{#if col.id === 'done' && all.length > DONE_LIMIT}
						<Button variant="ghost" onclick={() => (showAllDone = !showAllDone)}>
							{showAllDone ? 'Show fewer' : `Show all (${all.length})`}
						</Button>
					{/if}
				{:else}
					<p class="ez-board__empty ez-muted">Nothing here</p>
				{/if}
			</section>
		{/each}
	</div>
{/if}
