<script lang="ts">
	import {
		buildSteps,
		type TerminalProgress,
		type TerminalStepState
	} from '#lib/terminal-steps.ts';

	let { progress }: { progress: TerminalProgress } = $props();

	const steps = $derived(buildSteps(progress));
	const marks: Record<TerminalStepState, string> = {
		pending: '○',
		'in-progress': '◐',
		done: '✓',
		failed: '✗'
	};
	const words: Record<TerminalStepState, string> = {
		pending: 'Pending',
		'in-progress': 'In progress',
		done: 'Done',
		failed: 'Failed'
	};
</script>

<ol class="checklist" aria-label="Connection progress">
	{#each steps as step (step.id)}
		<li class="step {step.state}" data-state={step.state}>
			<span class="mark" aria-hidden="true">{marks[step.state]}</span>
			<span class="label">{step.label}</span>
			<span class="state">{words[step.state]}</span>
			{#if step.message}<span class="message muted">{step.message}</span>{/if}
		</li>
	{/each}
</ol>
