<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import Select from '#lib/components/form/Select.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import PlanDialog from '#lib/components/PlanDialog.svelte';
	import ReviewDialog from '#lib/components/ReviewDialog.svelte';
	import TaskBoard from '#lib/components/TaskBoard.svelte';
	import { getNeedsYou } from '#lib/needs-you.svelte.ts';
	import { listProjects } from '#lib/projects.remote.ts';
	import { applyItemChange, boardItems } from '#lib/work-items.ts';
	import { getWorkItem, getWorkItemPlan, listWorkItems, submitAction } from '#lib/work.remote.ts';

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

	// "Updated" reads as time since; one timer for the whole board keeps every card current.
	let now = $state(Date.now());
	$effect(() => {
		const timer = setInterval(() => (now = Date.now()), 60_000);
		return () => clearInterval(timer);
	});

	/** The queue and the listed items as one set: a task is on the board once, in the column its state says. */
	const board = $derived(boardItems(needsYou.items, items.current ?? [], project));

	// Dialogs follow the live item, so a card that moves on closes or updates its dialog.
	let planId = $state<number | null>(null);
	let reviewId = $state<number | null>(null);
	let planOpen = $state(false);
	let reviewOpen = $state(false);
	const planItem = $derived(board.find((w) => w.id === planId) ?? null);
	const reviewItem = $derived(board.find((w) => w.id === reviewId) ?? null);

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
</script>

<svelte:head><title>Work · enzarb</title></svelte:head>
<PageHeader
	title="Work"
	description="Issues, change requests and mentions your projects are handling. Columns on the left of the board need a plan approved; on the right, a change merged."
/>

<div class="ez-toolbar">
	<div class="ez-toolbar__item">
		<Select label="Project" value={project ?? ''} onchange={pick} {options} />
	</div>
</div>

<svelte:boundary>
	{#if items.error}
		<Notice tone="error">Could not load tasks.</Notice>
	{:else}
		{#if needsYou.failed && !needsYou.loaded}
			<Notice tone="error">Could not load the queue of tasks waiting on you.</Notice>
		{/if}
		<TaskBoard
			items={board}
			{project}
			{now}
			loading={(items.loading && !items.current) || (!needsYou.loaded && !needsYou.failed)}
			onaction={(id, action) => needsYou.act(id, action)}
			onplan={(w) => {
				planId = w.id;
				planOpen = true;
			}}
			onreview={(w) => {
				reviewId = w.id;
				reviewOpen = true;
			}}
		/>
	{/if}
	{#snippet failed()}<Notice tone="error">Could not load tasks.</Notice>{/snippet}
</svelte:boundary>

<PlanDialog
	bind:open={planOpen}
	item={planItem}
	load={(id) => getWorkItemPlan(id)}
	onaction={(id, action) => needsYou.act(id, action)}
	oncomment={(id, body) => submitAction({ id, action: 'comment', data: { body } })}
/>
<ReviewDialog
	bind:open={reviewOpen}
	item={reviewItem}
	load={(id) => getWorkItem(id)}
	onmerge={(id, { deleteBranch }) =>
		submitAction({ id, action: 'merge', data: deleteBranch ? { deleteBranch } : undefined })}
/>
