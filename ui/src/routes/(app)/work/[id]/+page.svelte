<script lang="ts">
	import { page } from '$app/state';
	import JobTable from '#lib/components/JobTable.svelte';
	import PageHeading from '#lib/components/PageHeading.svelte';
	import WorkActions from '#lib/components/WorkActions.svelte';
	import WorkStateChip from '#lib/components/WorkStateChip.svelte';
	import WorkTimeline from '#lib/components/WorkTimeline.svelte';
	import { watchWorkItems } from '#lib/live.ts';
	import type { WorkAction } from '#lib/work-items.ts';
	import { getWorkItem, submitAction } from '#lib/work.remote.ts';

	const id = $derived(Number(page.params.id));
	const item = $derived(getWorkItem(id));

	$effect(() =>
		watchWorkItems((changed) => (changed === null || changed === id) && void item.refresh())
	);

	let pending = $state<WorkAction | null>(null);
	let actionError = $state('');

	async function act(action: WorkAction) {
		pending = action;
		actionError = '';
		try {
			await submitAction({ id, action });
			// The reducer applies the event asynchronously; the SSE stream refreshes the page then.
		} catch {
			actionError = `Could not ${action} this item. Its state may have changed.`;
			void item.refresh();
		} finally {
			pending = null;
		}
	}
</script>

<svelte:head><title>Work item {page.params.id} · enzarb</title></svelte:head>

<svelte:boundary>
	{#if item.current}
		{@const w = item.current}
		<PageHeading title={w.title} />
		<p class="muted">
			<WorkStateChip state={w.state} />
			· <code>{w.key}</code> · project <a href="/projects/{w.project}">{w.project}</a>
			· {w.workflow}
			{#if w.url}· <a href={w.url} rel="noopener noreferrer">View on GitHub</a>{/if}
		</p>

		<WorkActions actions={w.actions} {pending} onaction={act} />
		{#if actionError}<p class="field-error" role="alert">{actionError}</p>{/if}

		<section class="card stack">
			<h2>Timeline</h2>
			<WorkTimeline transitions={w.transitions} state={w.state} />
		</section>

		<section class="card stack">
			<h2>Jobs</h2>
			<JobTable jobs={w.jobs} />
		</section>
	{:else}
		<p class="muted">Loading work item…</p>
	{/if}
	{#snippet failed()}
		<PageHeading title="Work item not found" />
		<p><a href="/work">Back to work</a></p>
	{/snippet}
</svelte:boundary>
