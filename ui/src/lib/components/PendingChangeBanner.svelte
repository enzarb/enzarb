<script lang="ts">
	import type { PendingChange } from '#lib/project-types.ts';
	import { POD_SHAPE_QUEUED_COPY } from '#lib/pending-change.ts';
	import { pendingChangeSteps } from '#lib/projects.ts';
	import Notice from './ui/Notice.svelte';
	import StepList, { type StepItem } from './ui/StepList.svelte';

	const READY_HOLD_MS = 5000;

	/**
	 * A queued sandbox change: Queued, Waiting for N running jobs, Applying, Applied. The operator
	 * clears `pendingChange` on completion, so the last change is kept to show the applied state
	 * briefly before the banner hides.
	 */
	let { change, runningJobs }: { change?: PendingChange | null; runningJobs: number | null } =
		$props();

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
	</div>
{/if}
