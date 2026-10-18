<script lang="ts">
	import type { PendingChange } from '#lib/project-types.ts';
	import {
		POD_SHAPE_QUEUED_COPY,
		formatRemainingDrain,
		remainingDrain
	} from '#lib/pending-change.ts';
	import { pendingChangeSteps } from '#lib/projects.ts';
	import Notice from './ui/Notice.svelte';
	import RollNowButton from './RollNowButton.svelte';
	import StepList, { type StepItem } from './ui/StepList.svelte';

	const READY_HOLD_MS = 5000;

	/**
	 * A queued sandbox change: Queued, Waiting for N running jobs, Applying, Applied. The operator
	 * clears `pendingChange` on completion, so the last change is kept to show the applied state
	 * briefly before the banner hides.
	 */
	let {
		change,
		runningJobs,
		roll
	}: {
		change?: PendingChange | null;
		runningJobs: number | null;
		/** Owner action: restart now for this generation. Without it no button is shown. */
		roll?: (generation: number) => Promise<void>;
	} = $props();

	let last = $state<PendingChange | null>(null);
	let applied = $state(false);

	$effect(() => {
		if (change) {
			last = change;
			applied = false;
			return;
		}
		if (!last) return;
		applied = true;
		const id = setTimeout(() => {
			last = null;
			applied = false;
		}, READY_HOLD_MS);
		return () => clearTimeout(id);
	});

	let now = $state(Date.now());
	$effect(() => {
		if (!change?.deadline) return;
		now = Date.now();
		const id = setInterval(() => (now = Date.now()), 1000);
		return () => clearInterval(id);
	});
	const remaining = $derived(change ? remainingDrain(change.deadline, now) : null);

	const words = { pending: 'Pending', active: 'In progress', done: 'Done' } as const;
	const shown = $derived(change ?? (applied ? last : null));
	const items = $derived<StepItem[]>(
		pendingChangeSteps(change, runningJobs, applied).map((s) => ({
			id: s.id,
			status: s.state,
			label: s.label,
			stateText: words[s.state],
			attrs: { 'data-state': s.state }
		}))
	);
	const since = $derived(shown?.since ? new Date(shown.since) : null);
	const sinceValid = $derived(since !== null && !Number.isNaN(since.getTime()));
</script>

{#if shown}
	<div class="ez-stack" aria-live="polite" data-testid="pending-change">
		<Notice
			tone={applied && !change ? 'success' : 'info'}
			title={applied && !change ? 'Sandbox change applied' : 'Sandbox change queued'}
		>
			{#if shown.reason}{shown.reason}{/if}
			{#if change}{POD_SHAPE_QUEUED_COPY}{/if}
			{#if sinceValid && since}
				<span class="ez-muted"
					>since <time datetime={shown.since}>{since.toLocaleString()}</time></span
				>
			{/if}
		</Notice>
		<StepList {items} label="Sandbox change progress" />
		{#if change && roll}
			<div>
				<RollNowButton generation={change.generation} {roll} />
				{#if remaining !== null}
					<span class="ez-muted" data-testid="drain-remaining"
						>{formatRemainingDrain(remaining)}</span
					>
				{/if}
			</div>
		{:else if change && remaining !== null}
			<div class="ez-muted" data-testid="drain-remaining">{formatRemainingDrain(remaining)}</div>
		{/if}
	</div>
{/if}
