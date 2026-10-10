<script lang="ts">
	import DataTable, { type Column } from '../DataTable.svelte';
	let {
		items,
		loading = false,
		emptyLevel
	}: {
		items: { id: number; name: string }[];
		loading?: boolean;
		emptyLevel?: 2 | 3 | 4;
	} = $props();
	const columns: Column<{ id: number; name: string }>[] = [
		{ id: 'name', header: 'Name', cell: name }
	];
</script>

{#snippet name(r: { id: number; name: string })}{r.name}{/snippet}
{#snippet actions(r: { id: number; name: string })}<button>Open {r.name}</button>{/snippet}

<DataTable
	label="Things"
	{items}
	{columns}
	rowKey={(r) => r.id}
	rowActions={actions}
	{loading}
	{emptyLevel}
	emptyTitle="No things"
	emptyDescription="Make one."
/>
