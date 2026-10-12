<script lang="ts">
	import type { CheckResult, CheckStatus, Readiness } from '#lib/work-items.ts';
	import Notice from './ui/Notice.svelte';
	import StepList, { type StepItem, type StepStatus } from './ui/StepList.svelte';

	/**
	 * Merge readiness of a change request: three engine-computed checks, or while Merging the fixed
	 * merge steps. Fixed glyphs, never a spinner. `merge_error` (why the last attempt did not
	 * happen) shows as a notice; in Reviewing it is the only thing shown.
	 */
	let { readiness, state }: { readiness: Readiness | null; state: string } = $props();

	const status: Record<CheckStatus, StepStatus> = {
		ok: 'done',
		pending: 'pending',
		failed: 'failed',
		unknown: 'pending'
	};
	const words: Record<CheckStatus, string> = {
		ok: 'Ok',
		pending: 'Pending',
		failed: 'Failed',
		unknown: 'Unknown'
	};

	const check = (id: string, label: string, c: CheckResult | undefined): StepItem => {
		const s = c?.status ?? 'unknown';
		return {
			id,
			status: status[s],
			label,
			stateText: words[s],
			message: c?.detail ?? undefined,
			attrs: { 'data-state': s }
		};
	};

	const checks = $derived<StepItem[]>([
		check('ci', 'CI on head', readiness?.ci),
		check('review', 'enzarb review approved', readiness?.review),
		check('mergeable', 'No conflicts, mergeable', readiness?.mergeable)
	]);

	// `state_data` carries no step marker: the first step is active, the others wait.
	const merging: StepItem[] = [
		{ id: 're-check', status: 'active', label: 'Re-checking readiness', stateText: 'In progress' },
		{ id: 'squash', status: 'pending', label: 'Squash-merging', stateText: 'Pending' },
		{ id: 'done', status: 'pending', label: 'Done', stateText: 'Pending' }
	];
</script>

{#if readiness?.mergeError}
	<Notice tone="warning" title="Last merge attempt did not happen">{readiness.mergeError}</Notice>
{/if}
{#if state === 'Merging'}
	<StepList items={merging} label="Merge progress" />
{:else if state !== 'Reviewing'}
	<StepList items={checks} label="Merge readiness" />
{/if}
