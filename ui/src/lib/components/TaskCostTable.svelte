<script lang="ts">
	import type { TaskUsage } from '#lib/server/metrics-service.ts';
	import { formatCost, formatTokens, totalInput } from '#lib/usage.ts';
	import DataTable, { type Column } from './ui/DataTable.svelte';

	/** Usage per task of a window; sortable, with a dash for what was not reported. */
	let { tasks }: { tasks: TaskUsage[] } = $props();

	const columns: Column<TaskUsage>[] = [
		{ id: 'task', header: 'Task', cell: key, sortValue: (t) => t.key },
		{ id: 'state', header: 'State', cell: state, sortValue: (t) => t.state },
		{
			id: 'in',
			header: 'Input',
			cell: input,
			numeric: true,
			sortValue: (t) => totalInput(t.usage)
		},
		{
			id: 'out',
			header: 'Output',
			cell: output,
			numeric: true,
			sortValue: (t) => t.usage?.outputTokens ?? null
		},
		{
			id: 'cost',
			header: 'Cost',
			cell: cost,
			numeric: true,
			sortValue: (t) => t.usage?.costUsd ?? null
		}
	];
</script>

{#snippet key(t: TaskUsage)}<span class="ez-mono">{t.key}</span>{/snippet}
{#snippet state(t: TaskUsage)}{t.state}{/snippet}
{#snippet input(t: TaskUsage)}{formatTokens(totalInput(t.usage))}{/snippet}
{#snippet output(t: TaskUsage)}{formatTokens(t.usage?.outputTokens)}{/snippet}
{#snippet cost(t: TaskUsage)}{formatCost(t.usage?.costUsd)}{/snippet}

<DataTable
	items={tasks}
	{columns}
	rowKey={(t) => t.key}
	label="Cost per task"
	emptyTitle="No usage in this window"
/>
