<script lang="ts">
	import type { NeedsYou, WorkAction, WorkItemSummary } from '#lib/work-items.ts';
	import NeedsYouRow from './NeedsYouRow.svelte';

	let {
		items,
		onaction,
		loadPlan
	}: {
		items: WorkItemSummary[];
		onaction: (id: number, action: WorkAction) => Promise<void>;
		loadPlan: (id: number) => Promise<string | null>;
	} = $props();

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
</script>

{#if items.length}
	<ul class="ez-needs" aria-label="Needs you">
		{#each items as item (item.id)}
			<NeedsYouRow
				item={item as WorkItemSummary & { needs: NeedsYou }}
				pending={pending?.id === item.id ? pending.action : null}
				error={errors[item.id] ?? ''}
				onaction={(a) => act(item.id, a)}
				{loadPlan}
			/>
		{/each}
	</ul>
{:else}
	<p class="ez-muted">Nothing is waiting on you.</p>
{/if}
