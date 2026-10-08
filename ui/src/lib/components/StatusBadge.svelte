<script lang="ts">
	import type { StepState } from '#lib/conditions.ts';

	let { state }: { state: StepState } = $props();
	const text: Record<StepState, string> = {
		pending: 'Pending',
		'in-progress': 'Starting',
		done: 'Ready',
		failed: 'Failed'
	};
</script>

<span class="badge {state}">{text[state]}</span>
