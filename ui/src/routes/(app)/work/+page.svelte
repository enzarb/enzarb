<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import Select from '#lib/components/form/Select.svelte';
	import NeedsYouQueue from '#lib/components/NeedsYouQueue.svelte';
	import DataTable, { type Column } from '#lib/components/ui/DataTable.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import WorkStateChip from '#lib/components/WorkStateChip.svelte';
	import { getNeedsYou } from '#lib/needs-you.svelte.ts';
	import { listProjects } from '#lib/projects.remote.ts';
	import Section from '#lib/components/ui/Section.svelte';
	import { applyItemChange, isTerminal, type WorkItemSummary } from '#lib/work-items.ts';
	import { getWorkItem, getWorkItemPlan, listWorkItems } from '#lib/work.remote.ts';

	const project = $derived(page.url.searchParams.get('project') ?? undefined);
	const items = $derived(listWorkItems(project));
	const projects = listProjects();
	const needsYou = getNeedsYou();

	/** Patch one changed row in place; any failure falls back to reloading the whole list. */
	async function applyChange(id: number) {
		const query = items;
		try {
			const changed = await getWorkItem(id);
			if (query !== items || !query.current) return void query.refresh();
			query.set(applyItemChange(query.current, changed, project));
		} catch {
			void query.refresh();
		}
	}

	// The layout owns the live socket; this page only listens to it.
	$effect(() =>
		needsYou.onChange((changed) => void (changed === null ? items.refresh() : applyChange(changed)))
	);

	const queue = $derived(needsYou.items.filter((w) => !project || w.project === project));
	/** Items already in the queue are not listed twice. */
	const rest = $derived((items.current ?? []).filter((w) => !w.needs));
	const inProgress = $derived(rest.filter((w) => !isTerminal(w.state)));
	const done = $derived(rest.filter((w) => isTerminal(w.state)));

	const options = $derived.by(() => {
		const list = (projects.current ?? []).map((p) => ({ value: p.name, label: p.displayName }));
		// keep the selection visible while the project list is still loading
		if (project && !list.some((o) => o.value === project))
			list.push({ value: project, label: project });
		return [{ value: '', label: 'All projects' }, ...list];
	});

	function pick(value: string) {
		void goto(value ? `/work?project=${encodeURIComponent(value)}` : '/work');
	}

	const columns: Column<WorkItemSummary>[] = [
		{ id: 'item', header: 'Item', cell: item },
		{ id: 'project', header: 'Project', cell: projectCell },
		{ id: 'workflow', header: 'Workflow', cell: workflow },
		{ id: 'state', header: 'State', cell: state },
		{ id: 'updated', header: 'Updated', cell: updated }
	];
</script>

{#snippet item(w: WorkItemSummary)}
	<a href="/work/{w.id}">{w.title}</a> <code class="ez-muted">{w.key}</code>
{/snippet}
{#snippet projectCell(w: WorkItemSummary)}{w.project}{/snippet}
{#snippet workflow(w: WorkItemSummary)}{w.workflow}{/snippet}
{#snippet state(w: WorkItemSummary)}
	<WorkStateChip state={w.state} />
	{#if w.waiting}<span class="ez-muted">{w.waiting}</span>{/if}
{/snippet}
{#snippet updated(w: WorkItemSummary)}
	<time datetime={w.updatedAt}>{w.updatedAt.slice(0, 16).replace('T', ' ')}</time>
{/snippet}

<svelte:head><title>Work · enzarb</title></svelte:head>
<PageHeader
	title="Work"
	description="Issues, change requests and mentions your projects are handling."
/>

<div class="ez-toolbar">
	<div class="ez-toolbar__item">
		<Select label="Project" value={project ?? ''} onchange={pick} {options} />
	</div>
</div>

<Section title="Needs you" description="Waiting on you: approve, merge, unblock or retry.">
	{#if needsYou.failed && !needsYou.loaded}
		<Notice tone="error">Could not load the queue.</Notice>
	{:else}
		<NeedsYouQueue
			items={queue}
			onaction={(id, action) => needsYou.act(id, action)}
			loadPlan={(id) => getWorkItemPlan(id)}
		/>
	{/if}
</Section>

<svelte:boundary>
	{#if items.error}
		<Notice tone="error">Could not load work items.</Notice>
	{:else}
		<DataTable
			label="In progress"
			items={inProgress}
			{columns}
			rowKey={(w) => w.id}
			loading={items.loading && !items.current}
			loadingLabel="Loading work items"
			emptyTitle="Nothing in progress"
			emptyDescription="Work appears here when an issue, change request or mention reaches a project."
		/>
		{#if done.length}
			<details class="ez-done">
				<summary>Done ({done.length})</summary>
				<DataTable
					label="Done"
					items={done}
					{columns}
					rowKey={(w) => w.id}
					emptyTitle="Nothing done"
				/>
			</details>
		{/if}
	{/if}
	{#snippet failed()}<Notice tone="error">Could not load work items.</Notice>{/snippet}
</svelte:boundary>
