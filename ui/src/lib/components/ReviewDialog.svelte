<script lang="ts">
	import { untrack } from 'svelte';
	import {
		githubRefLabel,
		isTerminal,
		type WorkItemDetail,
		type WorkItemSummary
	} from '#lib/work-items.ts';
	import MergeReadiness from './MergeReadiness.svelte';
	import Button from './form/Button.svelte';
	import Checkbox from './form/Checkbox.svelte';
	import ConfirmDialog from './ui/ConfirmDialog.svelte';
	import Dialog from './ui/Dialog.svelte';
	import LinkButton from './ui/LinkButton.svelte';
	import Notice from './ui/Notice.svelte';
	import Skeleton from './ui/Skeleton.svelte';

	/**
	 * A change request in a dialog: merge readiness, enzarb's latest review, the pull request link,
	 * and the squash merge with an optional "delete the branch afterwards". `load` returns the
	 * item's detail; it runs when the dialog opens and again whenever `item` changes (a live
	 * update), so a refused merge shows its reason here (`MergeReadiness`). The merge is offered
	 * only while the item can be merged. The dialog closes once the item is finished.
	 */
	let {
		open = $bindable(false),
		item,
		load,
		onmerge
	}: {
		open?: boolean;
		item: WorkItemSummary | null;
		load: (id: number) => Promise<WorkItemDetail>;
		onmerge: (id: number, options: { deleteBranch: boolean }) => Promise<void>;
	} = $props();

	const id = $derived(item?.id ?? null);
	const updatedAt = $derived(item?.updatedAt ?? '');

	let detail = $state<WorkItemDetail | null>(null);
	let failed = $state(false);
	let deleteBranch = $state(true);
	let confirmOpen = $state(false);
	let requested = $state(false);
	let error = $state('');

	// A new item starts over; a live update of the same item only reloads its detail.
	$effect(() => {
		if (!open || id === null) return;
		untrack(() => {
			detail = null;
			failed = false;
			requested = false;
			error = '';
			deleteBranch = true;
		});
	});

	$effect(() => {
		if (!open || id === null) return;
		void updatedAt;
		const wanted = id;
		let current = true;
		load(wanted).then(
			(d) => {
				if (!current) return;
				detail = d;
				failed = false;
			},
			() => {
				if (current) failed = true;
			}
		);
		return () => (current = false);
	});

	$effect(() => {
		if (open && requested && item && isTerminal(item.state)) open = false;
	});

	const canMerge = $derived(!!detail?.actions.includes('merge') && !requested);
	const review = $derived(
		[...(detail?.jobs ?? [])].reverse().find((j) => j.kind === 'agent_review' && j.outputHtml)
	);

	async function merge() {
		if (id === null) return;
		error = '';
		try {
			await onmerge(id, { deleteBranch });
			requested = true;
		} catch {
			error = 'Could not merge this change request. Its state may have changed.';
		}
	}
</script>

<Dialog bind:open wide title={item ? `Review: ${item.title}` : 'Review'} description={item?.key}>
	{#if detail}
		{@const w = detail}
		{#if w.url}
			<p>
				<a
					href={w.url}
					target="_blank"
					rel="noopener noreferrer"
					title="View on GitHub (opens in a new tab)"
					>{githubRefLabel(w.url, w.singleRepo) ?? 'View on GitHub'}</a
				>
				<span class="ez-muted">· line comments and the diff are on GitHub</span>
			</p>
		{/if}
		<MergeReadiness readiness={w.readiness} state={w.state} />
		{#if review?.outputHtml}
			<div class="ez-dialog__section">
				<h4 class="ez-dialog__heading">Latest review</h4>
				<!-- Sanitized on the server (sanitize-html allow-list) before it is sent. -->
				<!-- eslint-disable-next-line svelte/no-at-html-tags -->
				<div class="ez-markdown ez-dialog__scroll">{@html review.outputHtml}</div>
			</div>
		{/if}
		{#if requested}
			<Notice tone="success">Merge requested. This closes when the merge is done.</Notice>
		{:else if !canMerge}
			<Notice tone="info">It can be merged once review and checks pass.</Notice>
		{/if}
		<div class="ez-dialog__section">
			<Checkbox
				label="Delete branch after merge"
				help="Only enzarb's own branches are deleted, and not while other change requests are stacked on it."
				bind:checked={deleteBranch}
				disabled={!canMerge}
			/>
		</div>
	{:else if failed}
		<Notice tone="error">Could not load this change request.</Notice>
	{:else}
		<Skeleton label="Loading review" rows={5} />
	{/if}
	{#if error}<Notice tone="error">{error}</Notice>{/if}
	{#snippet footer()}
		<Button variant="ghost" onclick={() => (open = false)}>Close</Button>
		{#if item?.url}
			<LinkButton
				href={`${item.url}/files`}
				target="_blank"
				rel="noopener noreferrer"
				variant="secondary">Review on GitHub</LinkButton
			>
		{/if}
		<Button variant="primary" disabled={!canMerge} onclick={() => (confirmOpen = true)}
			>Merge (squash)</Button
		>
	{/snippet}
</Dialog>

<ConfirmDialog
	bind:open={confirmOpen}
	tone="primary"
	title="Merge this change request?"
	description={`“${item?.title ?? ''}” will be squash-merged${deleteBranch ? ' and its branch deleted' : ''}.`}
	confirmLabel="Merge (squash)"
	onconfirm={merge}
/>
