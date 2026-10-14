<script lang="ts" module>
	import type { Snippet } from 'svelte';

	export interface Column<T> {
		id: string;
		header: string;
		/** Renders the cell for one row. */
		cell: Snippet<[T]>;
		/**
		 * Makes the column sortable: the header becomes a button and rows are ordered by this value.
		 * Null (unknown) sorts last in either direction.
		 */
		sortValue?: (item: T) => number | string | null;
		/** Right-align the column (numbers). */
		numeric?: boolean;
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

	/** The column rows are sorted by, if any; the first click on a column sorts descending. */
	let sort = $state<{ id: string; dir: 'asc' | 'desc' } | null>(null);

	function toggle(id: string) {
		sort = sort?.id === id && sort.dir === 'desc' ? { id, dir: 'asc' } : { id, dir: 'desc' };
	}

	const rows = $derived.by(() => {
		const col = sort ? columns.find((c) => c.id === sort!.id) : undefined;
		if (!sort || !col?.sortValue) return items;
		const { dir } = sort;
		const value = col.sortValue;
		const sign = dir === 'asc' ? 1 : -1;
		return items
			.map((item, i) => ({ item, i, v: value(item) }))
			.sort((a, b) => {
				if (a.v === null || b.v === null) return a.v === b.v ? a.i - b.i : a.v === null ? 1 : -1;
				const c =
					typeof a.v === 'number' && typeof b.v === 'number'
						? a.v - b.v
						: `${a.v}`.localeCompare(`${b.v}`);
				return c === 0 ? a.i - b.i : sign * c;
			})
			.map((x) => x.item);
	});

	const ariaSort = (id: string) =>
		sort?.id === id ? (sort.dir === 'asc' ? 'ascending' : 'descending') : undefined;

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
						{#each columns as c (c.id)}
							<th
								scope="col"
								role="columnheader"
								class:ez-table__num={c.numeric}
								aria-sort={c.sortValue ? (ariaSort(c.id) ?? 'none') : undefined}
							>
								{#if c.sortValue}
									<button type="button" class="ez-table__sort" onclick={() => toggle(c.id)}>
										{c.header}
										<span aria-hidden="true"
											>{sort?.id === c.id ? (sort.dir === 'asc' ? '▲' : '▼') : '↕'}</span
										>
									</button>
								{:else}
									{c.header}
								{/if}
							</th>
						{/each}
						{#if rowActions}<th scope="col" role="columnheader"
								><span class="ez-sr-only">Actions</span></th
							>{/if}
					</tr>
				</thead>
				<tbody {...role.rowgroup}>
					{#each rows as item (rowKey(item))}
						<tr {...role.row}>
							{#each columns as c (c.id)}
								<td role="cell" data-label={c.header} class:ez-table__num={c.numeric}
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
