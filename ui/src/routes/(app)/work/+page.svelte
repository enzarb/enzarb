<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import PageHeading from '#lib/components/PageHeading.svelte';
	import WorkStateChip from '#lib/components/WorkStateChip.svelte';
	import { watchWorkItems } from '#lib/live.ts';
	import { listProjects } from '#lib/projects.remote.ts';
	import { listWorkItems } from '#lib/work.remote.ts';

	const project = $derived(page.url.searchParams.get('project') ?? undefined);
	const items = $derived(listWorkItems(project));
	const projects = listProjects();

	$effect(() => watchWorkItems(() => void items.refresh()));

	function pick(e: Event) {
		const value = (e.currentTarget as HTMLSelectElement).value;
		void goto(value ? `/work?project=${encodeURIComponent(value)}` : '/work');
	}
</script>

<svelte:head><title>Work · enzarb</title></svelte:head>

<PageHeading title="Work" />

<p>
	<label>
		Project
		<select value={project ?? ''} onchange={pick}>
			<option value="">All projects</option>
			{#each projects.current ?? [] as p (p.name)}
				<option value={p.name}>{p.displayName}</option>
			{/each}
		</select>
	</label>
</p>

<svelte:boundary>
	{#if items.error}
		<p class="field-error">Could not load work items.</p>
	{:else if items.loading && !items.current}
		<p class="muted">Loading work items…</p>
	{:else if items.current?.length}
		<table class="table">
			<thead>
				<tr><th>Item</th><th>Project</th><th>Workflow</th><th>State</th><th>Updated</th></tr>
			</thead>
			<tbody>
				{#each items.current as w (w.id)}
					<tr>
						<td><a href="/work/{w.id}">{w.title}</a> <code class="muted">{w.key}</code></td>
						<td>{w.project}</td>
						<td>{w.workflow}</td>
						<td><WorkStateChip state={w.state} /></td>
						<td><time datetime={w.updatedAt}>{w.updatedAt.slice(0, 16).replace('T', ' ')}</time></td
						>
					</tr>
				{/each}
			</tbody>
		</table>
	{:else}
		<p class="muted">No work items yet.</p>
	{/if}
	{#snippet failed()}<p class="field-error">Could not load work items.</p>{/snippet}
</svelte:boundary>
