<script lang="ts">
	import { buildChecklist, type StepState } from '#lib/conditions.ts';
	import type { Condition } from '#lib/project-types.ts';

	let { conditions }: { conditions: Condition[] } = $props();

	const steps = $derived(buildChecklist(conditions));
	const marks: Record<StepState, string> = {
		pending: '○',
		'in-progress': '◐',
		done: '✓',
		failed: '✗'
	};
	const words: Record<StepState, string> = {
		pending: 'Pending',
		'in-progress': 'In progress',
		done: 'Done',
		failed: 'Failed'
	};
</script>

<ol class="checklist" aria-label="Boot progress">
	{#each steps as step (step.type)}
		<li class="step {step.state}" data-state={step.state}>
			<span class="mark" aria-hidden="true">{marks[step.state]}</span>
			<span class="label">{step.label}</span>
			<span class="state">{words[step.state]}</span>
			{#if step.message}<span class="message muted">{step.message}</span>{/if}
		</li>
	{/each}
</ol>
