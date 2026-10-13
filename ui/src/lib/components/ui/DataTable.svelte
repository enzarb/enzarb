<script lang="ts" module>
	import type { Snippet } from 'svelte';

	export interface Column<T> {
		id: string;
		header: string;
		/** Renders the cell for one row. */
		cell: Snippet<[T]>;
	}
</script>

<script lang="ts" generics="T">
	import Card from './Card.svelte';
	import EmptyState from './EmptyState.svelte';
	import Skeleton from './Skeleton.svelte';

	/**
	 * Table with an empty state, loading skeleton and optional per-row actions. Columns are
	 * `{ id, header, cell }` with the cell a snippet, so the page owns the content and this owns
	 * the look. Below 40rem each row collapses to stacked "label: value" pairs (cells get a
	 * `data-label`). `framed={false}` drops the surrounding Card (when already inside one).
	 */
	let {
		items,
		columns,
		rowKey,
		rowActions,
		loading = false,
		loadingLabel = 'Loading',
		emptyTitle = 'Nothing here yet',
		emptyDescription,
		emptyActions,
		emptyLevel = 2,
		label,
		framed = true
	}: {
		items: T[];
		columns: Column<T>[];
		rowKey: (item: T) => string | number;
		rowActions?: Snippet<[T]>;
		loading?: boolean;
		loadingLabel?: string;
		emptyTitle?: string;
		emptyDescription?: string;
		emptyActions?: Snippet;
		/** Heading level of the empty-state title (below the surrounding heading). */
		emptyLevel?: 2 | 3 | 4;
		/** Accessible name of the table. */
		label: string;
		framed?: boolean;
	} = $props();

	// The phone layout sets display:block/grid on table parts, which makes WebKit drop their native
	// semantics; repeating the roles keeps them. (Spread, so the redundant-role lint stays quiet.)
	const role = {
		table: { role: 'table' },
		rowgroup: { role: 'rowgroup' },
		row: { role: 'row' }
	} as const;
</script>

{#snippet content()}
	{#if loading}
		<Skeleton label={loadingLabel} />
	{:else if items.length === 0}
		<EmptyState
			title={emptyTitle}
			description={emptyDescription}
			actions={emptyActions}
			level={emptyLevel}
		/>
	{:else}
		<div class="ez-table-wrap">
			<table class="ez-table" {...role.table} aria-label={label}>
				<thead {...role.rowgroup}>
					<tr {...role.row}>
						{#each columns as c (c.id)}<th scope="col" role="columnheader">{c.header}</th>{/each}
						{#if rowActions}<th scope="col" role="columnheader"
								><span class="ez-sr-only">Actions</span></th
							>{/if}
					</tr>
				</thead>
				<tbody {...role.rowgroup}>
					{#each items as item (rowKey(item))}
						<tr {...role.row}>
							{#each columns as c (c.id)}
								<td role="cell" data-label={c.header}
									><div class="ez-table__cell">{@render c.cell(item)}</div></td
								>
							{/each}
							{#if rowActions}
								<td role="cell" class="ez-table__actions"
									><div class="ez-cluster">{@render rowActions(item)}</div></td
								>
							{/if}
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
{/snippet}

{#if framed}
	<Card flush>{@render content()}</Card>
{:else}
	{@render content()}
{/if}
