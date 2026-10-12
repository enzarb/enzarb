<script lang="ts">
	import { page } from '$app/state';
	import JobTable from '#lib/components/JobTable.svelte';
	import MergeReadiness from '#lib/components/MergeReadiness.svelte';
	import ConfirmDialog from '#lib/components/ui/ConfirmDialog.svelte';
	import StepList from '#lib/components/ui/StepList.svelte';
	import LinkButton from '#lib/components/ui/LinkButton.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import WorkActions from '#lib/components/WorkActions.svelte';
	import WorkStateChip from '#lib/components/WorkStateChip.svelte';
	import WorkTimeline from '#lib/components/WorkTimeline.svelte';
	import { watchWorkItems } from '#lib/live.ts';
	import { shouldRefresh, type WorkAction } from '#lib/work-items.ts';
	import { getWorkItem, submitAction } from '#lib/work.remote.ts';

	const id = $derived(Number(page.params.id));
	const item = $derived(getWorkItem(id));

	$effect(() => watchWorkItems((changed) => shouldRefresh(changed, id) && void item.refresh()));

	let pending = $state<WorkAction | null>(null);
	let actionError = $state('');
	let confirmOpen = $state(false);
	/** The item's state and merge_error when merge was requested; the hint ends when they change. */
	let mergeRequestedAt = $state<string | null>(null);

	const MERGE_STATES = ['Reviewing', 'AwaitingChanges', 'ReadyToMerge', 'Merging'];
	const snapshot = $derived(
		item.current ? `${item.current.state}|${item.current.readiness?.mergeError ?? ''}` : null
	);
	const mergeRequested = $derived(mergeRequestedAt !== null && mergeRequestedAt === snapshot);

	function request(action: WorkAction) {
		if (action === 'merge') confirmOpen = true;
		else void act(action);
	}

	async function act(action: WorkAction) {
		pending = action;
		actionError = '';
		try {
			const before = snapshot;
			await submitAction({ id, action });
			if (action === 'merge') mergeRequestedAt = before;
			// The reducer applies the event asynchronously; the live stream refreshes the page then.
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
		<PageHeader title={w.title}>
			{#snippet meta()}
				<WorkStateChip state={w.state} />
				<code>{w.key}</code>
				<span class="ez-muted">project <a href="/projects/{w.project}">{w.project}</a></span>
				<span class="ez-muted">{w.workflow}</span>
				{#if w.url}<a href={w.url} rel="noopener noreferrer">View on GitHub</a>{/if}
			{/snippet}
			{#snippet actions()}
				<WorkActions actions={w.actions} {pending} onaction={request} />
			{/snippet}
		</PageHeader>
		{#if actionError}<Notice tone="error">{actionError}</Notice>{/if}

		{#if w.readiness && MERGE_STATES.includes(w.state) && (w.state !== 'Reviewing' || w.readiness.mergeError)}
			<Section title="Merge readiness">
				<div class="ez-merge-readiness">
					{#if mergeRequested}
						<StepList
							label="Merge request"
							items={[
								{ id: 'requested', status: 'done', label: 'Merge requested', stateText: 'Sent' }
							]}
						/>
					{/if}
					<MergeReadiness readiness={w.readiness} state={w.state} />
				</div>
			</Section>
		{/if}
		<ConfirmDialog
			bind:open={confirmOpen}
			tone="primary"
			title="Merge this change request?"
			description="“{w.title}” will be squash-merged. The engine uses a Conventional Commit title derived from this one (a type prefix is added if missing)."
			confirmLabel="Merge (squash)"
			onconfirm={() => act('merge')}
		/>

		<Section title="Timeline">
			<WorkTimeline transitions={w.transitions} state={w.state} />
		</Section>
		<Section title="Jobs" flush>
			<JobTable jobs={w.jobs} />
		</Section>
	{:else}
		<Skeleton label="Loading work item" rows={4} />
	{/if}
	{#snippet failed()}
		<PageHeader title="Work item not found" />
		<div><LinkButton href="/work">Back to work</LinkButton></div>
	{/snippet}
</svelte:boundary>
