<script lang="ts">
	import type { StepState } from '#lib/conditions.ts';
	import Badge from './ui/Badge.svelte';
	import { stepStatus } from './ui/status.ts';

	/** Overall Project state as a Badge. */
	let { state }: { state: StepState } = $props();
	const text: Record<StepState, string> = {
		pending: 'Pending',
		'in-progress': 'Starting',
		done: 'Ready',
		failed: 'Failed'
	};
	const s = $derived(stepStatus[state]);
</script>

<Badge tone={s.tone} hollow={s.hollow}>{text[state]}</Badge>
