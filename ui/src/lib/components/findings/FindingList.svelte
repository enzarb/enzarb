<script lang="ts">
	import EmptyState from '#lib/components/ui/EmptyState.svelte';
	import type { FindingRow } from '#lib/tasks.ts';
	import FindingItem from './FindingRow.svelte';

	/** Open liveness findings, or the healthy empty state. */
	let { findings, now = Date.now() }: { findings: FindingRow[]; now?: number } = $props();
</script>

{#if findings.length}
	<ul class="ez-findings">
		{#each findings as f (f.key)}
			<FindingItem finding={f} {now} />
		{/each}
	</ul>
{:else}
	<EmptyState title="No findings" description="The platform looks healthy." icon="check" />
{/if}
