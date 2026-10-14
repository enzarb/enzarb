<script lang="ts">
	import { cacheBreakdown, formatCost, formatTokens, totalInput } from '#lib/usage.ts';
	import { isTerminal, type WorkItemSummary } from '#lib/work-items.ts';
	import UsageFigure from './UsageFigure.svelte';
	import DataTable, { type Column } from './ui/DataTable.svelte';
	import WaitReason from './WaitReason.svelte';
	import WorkStateChip from './WorkStateChip.svelte';

	/**
	 * Work items with their token usage and cost (totals over each item's jobs). The usage columns
	 * sort; the cache breakdown is in the tooltip of the "In" figure. A dash means not reported.
	 */
	let {
		items,
		label,
		loading = false,
		loadingLabel,
		emptyTitle,
		emptyDescription
	}: {
		items: WorkItemSummary[];
		label: string;
		loading?: boolean;
		loadingLabel?: string;
		emptyTitle?: string;
		emptyDescription?: string;
	} = $props();

	const columns: Column<WorkItemSummary>[] = [
		{ id: 'item', header: 'Item', cell: item },
		{ id: 'project', header: 'Project', cell: projectCell },
		{ id: 'workflow', header: 'Workflow', cell: workflow },
		{ id: 'state', header: 'State', cell: state },
		{
			id: 'tokens-in',
			header: 'Tokens in',
			cell: tokensIn,
			numeric: true,
			sortValue: (w) => totalInput(w.usage)
		},
		{
			id: 'tokens-out',
			header: 'Tokens out',
			cell: tokensOut,
			numeric: true,
			sortValue: (w) => w.usage?.outputTokens ?? null
		},
		{
			id: 'cost',
			header: 'Cost',
			cell: cost,
			numeric: true,
			sortValue: (w) => w.usage?.costUsd ?? null
		},
		{ id: 'updated', header: 'Updated', cell: updated }
	];
</script>

{#snippet item(w: WorkItemSummary)}
	<a href="/task/{w.id}">{w.title}</a> <code class="ez-muted">{w.key}</code>
{/snippet}
{#snippet projectCell(w: WorkItemSummary)}{w.project}{/snippet}
{#snippet workflow(w: WorkItemSummary)}{w.workflow}{/snippet}
{#snippet state(w: WorkItemSummary)}
	<WorkStateChip state={w.state} />
	{#if !isTerminal(w.state)}<div class="ez-muted">
			<WaitReason wait={w.wait} url={w.url} />
		</div>{/if}
{/snippet}
{#snippet tokensIn(w: WorkItemSummary)}
	<UsageFigure
		value={formatTokens(totalInput(w.usage))}
		hint={w.usage ? cacheBreakdown(w.usage) : undefined}
	/>
{/snippet}
{#snippet tokensOut(w: WorkItemSummary)}
	<UsageFigure value={formatTokens(w.usage?.outputTokens)} />
{/snippet}
{#snippet cost(w: WorkItemSummary)}<UsageFigure value={formatCost(w.usage?.costUsd)} />{/snippet}
{#snippet updated(w: WorkItemSummary)}
	<time datetime={w.updatedAt}>{w.updatedAt.slice(0, 16).replace('T', ' ')}</time>
{/snippet}

<DataTable
	{label}
	{items}
	{columns}
	rowKey={(w) => w.id}
	{loading}
	{loadingLabel}
	{emptyTitle}
	{emptyDescription}
/>
