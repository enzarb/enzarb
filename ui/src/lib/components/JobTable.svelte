<script lang="ts">
	import type { JobView } from '#lib/work-items.ts';
	import Badge from './ui/Badge.svelte';
	import DataTable, { type Column } from './ui/DataTable.svelte';
	import { jobStatus } from './ui/status.ts';

	/** A work item's jobs (runs) with their result and rendered output. */
	let { jobs }: { jobs: JobView[] } = $props();

	const columns: Column<JobView>[] = [
		{ id: 'kind', header: 'Job', cell: kind },
		{ id: 'status', header: 'Status', cell: status },
		{ id: 'attempt', header: 'Attempts', cell: attempt },
		{ id: 'output', header: 'Output', cell: output }
	];
</script>

{#snippet kind(j: JobView)}<code>{j.kind}</code>{/snippet}
{#snippet status(j: JobView)}
	{@const s = jobStatus(j.status)}
	<Badge tone={s.tone} hollow={s.hollow}>{j.status}</Badge>
{/snippet}
{#snippet attempt(j: JobView)}{j.attempt}{/snippet}
{#snippet output(j: JobView)}
	{#if j.outputHtml}
		<!-- Sanitized on the server (sanitize-html allow-list) before it is sent. -->
		<!-- eslint-disable-next-line svelte/no-at-html-tags -->
		<div class="ez-markdown">{@html j.outputHtml}</div>
	{:else}
		<span class="ez-subtle">—</span>
	{/if}
{/snippet}

<DataTable
	items={jobs}
	{columns}
	rowKey={(j) => j.id}
	label="Jobs"
	emptyTitle="No jobs yet"
	emptyLevel={3}
	framed={false}
/>
