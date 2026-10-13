<script lang="ts">
	import {
		primaryAction,
		waitingFor,
		type NeedsYou,
		type WorkAction,
		type WorkItemSummary
	} from '#lib/work-items.ts';
	import PlanPreview from './PlanPreview.svelte';
	import WorkActions from './WorkActions.svelte';
	import ConfirmDialog from './ui/ConfirmDialog.svelte';

	let {
		item,
		now = Date.now(),
		pending = null,
		error = '',
		onaction,
		loadPlan
	}: {
		item: WorkItemSummary & { needs: NeedsYou };
		now?: number;
		pending?: WorkAction | null;
		error?: string;
		onaction: (action: WorkAction) => void;
		loadPlan: (id: number) => Promise<string | null>;
	} = $props();

	const kinds = {
		approval: 'Plan awaiting approval',
		merge: 'Ready to merge',
		blocked: 'Blocked',
		failed: 'Failed'
	} as const;

	const action = $derived(primaryAction(item.needs.category));
	let confirmOpen = $state(false);

	function request(a: WorkAction) {
		if (a === 'merge') confirmOpen = true;
		else onaction(a);
	}
</script>

<li class="ez-needs__row ez-needs__row--{item.needs.category}">
	<div class="ez-needs__head">
		<strong>{kinds[item.needs.category]}</strong>
		<a href="/work/{item.id}">{item.title}</a>
		<span class="ez-muted">
			{item.project} · <code>{item.key}</code> ·
			<time datetime={item.updatedAt}>waiting {waitingFor(item.updatedAt, now)}</time>
		</span>
		{#if item.url}<a href={item.url} rel="noopener noreferrer">View on GitHub</a>{/if}
	</div>
	{#if item.needs.reason}<p class="ez-needs__reason">{item.needs.reason}</p>{/if}
	{#if item.needs.category === 'approval'}
		<PlanPreview load={() => loadPlan(item.id)} />
	{/if}
	{#if error}<p class="ez-needs__error" role="alert">{error}</p>{/if}
	{#if action}
		<WorkActions actions={[action]} {pending} onaction={request} />
	{/if}
</li>

<ConfirmDialog
	bind:open={confirmOpen}
	tone="primary"
	title="Merge this change request?"
	description="“{item.title}” will be squash-merged."
	confirmLabel="Merge (squash)"
	onconfirm={() => onaction('merge')}
/>
