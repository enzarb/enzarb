<script lang="ts">
	import type { MergeStep } from '#lib/pull-review.ts';
	import StepList, { type StepItem } from '../ui/StepList.svelte';

	/**
	 * The Approve & merge checklist. While the request runs (`steps` empty) the first step is
	 * active; afterwards each step shows how it ended.
	 */
	let { steps, running = false }: { steps: MergeStep[]; running?: boolean } = $props();

	const LABELS = [
		'Check your write access',
		'Approve the pull request',
		'Check mergeability and checks',
		'Squash and merge'
	];
	const items = $derived<StepItem[]>(
		steps.length
			? steps.map((s) => ({
					id: s.id,
					status: s.result === 'done' ? 'done' : s.result === 'failed' ? 'failed' : 'pending',
					label: s.label,
					stateText: s.result === 'done' ? 'Done' : s.result === 'failed' ? 'Failed' : 'Not run',
					message: s.message
				}))
			: LABELS.map((label, i) => ({
					id: i,
					status: running && i === 0 ? ('active' as const) : ('pending' as const),
					label,
					stateText: running && i === 0 ? 'In progress' : 'Pending'
				}))
	);
</script>

<StepList {items} label="Approve and merge progress" />
