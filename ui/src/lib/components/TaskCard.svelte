<script lang="ts">
	import { timeSince } from '#lib/time-since.ts';
	import { cacheBreakdown, formatCost, formatTokens, totalInput } from '#lib/usage.ts';
	import {
		githubRefLabel,
		isTerminal,
		primaryAction,
		waitLinks,
		type WorkAction,
		type WorkItemSummary
	} from '#lib/work-items.ts';
	import UsageFigure from './UsageFigure.svelte';
	import WaitReason from './WaitReason.svelte';
	import WaitButtons from './WaitButtons.svelte';
	import WorkActions from './WorkActions.svelte';
	import Button from './form/Button.svelte';
	import ConfirmDialog from './ui/ConfirmDialog.svelte';
	import LinkButton from './ui/LinkButton.svelte';

	/**
	 * One task on the board: title, key, workflow, GitHub reference, why it waits, usage and time
	 * since the last update. A task waiting on the user also shows what it needs and the primary
	 * action inline (approve, merge, retry); plan approvals and change requests to review open
	 * the plan or review dialog. The project is shown only when the page is not filtered to one.
	 */
	let {
		item,
		project,
		now = Date.now(),
		pending = null,
		error = '',
		onaction,
		oncommand,
		onplan,
		onreview,
		highlighted = false,
		note = null,
		onhighlight
	}: {
		item: WorkItemSummary;
		/** The project the page is filtered to; its name is left off the card. */
		project?: string;
		now?: number;
		pending?: WorkAction | null;
		error?: string;
		onaction: (action: WorkAction) => void;
		/** A wait's declared button: post `/<command>` as the user. */
		oncommand?: (command: string) => void;
		onplan?: () => void;
		onreview?: () => void;
		/** Opened from a notification: accented, and handed to `onhighlight` to scroll and focus. */
		highlighted?: boolean;
		/** What changed since the notification, shown on a highlighted card. */
		note?: string | null;
		onhighlight?: (el: HTMLElement) => void;
	} = $props();

	// Fallback headings when workflows gave no specific reason text.
	const kinds = {
		approval: 'Plan awaiting approval',
		merge: 'Ready to merge',
		review: 'Waiting for your review',
		decision: 'Waiting on your decision',
		blocked: 'Blocked',
		failed: 'Failed',
		question: 'Questions awaiting your answer'
	} as const;

	const needs = $derived(item.needs ?? null);
	// A wait that declares buttons (enzarb's start gate) offers those instead of approve.
	const buttons = $derived(needs && oncommand ? (item.wait?.actions ?? []) : []);
	const action = $derived(needs && !buttons.length ? primaryAction(needs.category) : null);
	const links = $derived(needs ? waitLinks(item.wait ?? null, item.url) : []);
	const isPlan = $derived(needs?.category === 'approval' && !item.wait?.actions?.length);
	const isReview = $derived(
		item.workflow === 'change_request' &&
			(needs?.category === 'merge' || needs?.category === 'review')
	);
	let confirmOpen = $state(false);

	function request(a: WorkAction) {
		if (a === 'merge') confirmOpen = true;
		else onaction(a);
	}
</script>

<!-- tabindex is -1 or absent: a highlighted card takes programmatic focus only, never a tab stop -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<li
	class={[
		'ez-card',
		needs !== null && 'ez-card--needs',
		item.state === 'Cancelled' && 'ez-card--cancelled',
		highlighted && 'ez-card--highlight'
	]}
	tabindex={highlighted ? -1 : undefined}
	aria-current={highlighted ? 'true' : undefined}
	{@attach (el) => {
		if (highlighted) onhighlight?.(el);
	}}
>
	<a class="ez-card__title" href="/task/{item.id}">{item.title}</a>
	<p class="ez-card__meta ez-muted">
		<code>{item.key}</code> · {item.workflow}{#if !project && item.project}
			· <span class="ez-card__project">{item.project}</span>{/if}
		{#if item.url}
			·
			<a
				href={item.url}
				target="_blank"
				rel="noopener noreferrer"
				title="View on GitHub (opens in a new tab)"
				>{githubRefLabel(item.url, item.singleRepo) ?? 'View on GitHub'}</a
			>
		{/if}
		·
		<time datetime={item.updatedAt} title={item.updatedAt}>{timeSince(item.updatedAt, now)}</time>
	</p>
	{#if highlighted && note}<p class="ez-card__note">{note}</p>{/if}
	{#if needs}
		<p class="ez-card__needs"><strong>{needs.text ?? kinds[needs.category]}</strong></p>
		{#if needs.reason}<p class="ez-card__reason">{needs.reason}</p>{/if}
	{:else if !isTerminal(item.state)}
		<p class="ez-card__wait ez-muted"><WaitReason wait={item.wait} url={item.url} /></p>
	{/if}
	{#if item.usage}
		<p class="ez-card__usage ez-muted">
			In
			<UsageFigure value={formatTokens(totalInput(item.usage))} hint={cacheBreakdown(item.usage)} />
			· Out <UsageFigure value={formatTokens(item.usage.outputTokens)} /> ·
			<UsageFigure value={formatCost(item.usage.costUsd)} />
		</p>
	{/if}
	{#if error}<p class="ez-card__error" role="alert">{error}</p>{/if}
	{#if action || buttons.length || links.length || (isPlan && onplan) || (isReview && onreview)}
		<div class="ez-cluster">
			{#if isPlan && onplan}
				<Button variant="secondary" onclick={onplan}>View plan</Button>
			{/if}
			{#if isReview && onreview}
				<Button variant="secondary" onclick={onreview}>Review</Button>
			{/if}
			{#if action}
				<WorkActions actions={[action]} {pending} onaction={request} />
			{/if}
			{#if buttons.length && oncommand}
				<WaitButtons {buttons} disabled={pending !== null} {oncommand} />
			{/if}
			{#each links as l (l.href)}
				<LinkButton
					href={l.href}
					target="_blank"
					rel="noopener noreferrer"
					variant={action || l.label === 'Open PR' ? 'secondary' : 'primary'}>{l.label}</LinkButton
				>
			{/each}
		</div>
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
