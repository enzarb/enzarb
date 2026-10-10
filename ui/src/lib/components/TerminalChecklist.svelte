<script lang="ts">
	import {
		buildSteps,
		type TerminalProgress,
		type TerminalStepState
	} from '#lib/terminal-steps.ts';
	import StepList, { type StepItem, type StepStatus } from './ui/StepList.svelte';

	/** Connection progress of a sandbox terminal. */
	let { progress }: { progress: TerminalProgress } = $props();

	const status: Record<TerminalStepState, StepStatus> = {
		pending: 'pending',
		'in-progress': 'active',
		done: 'done',
		failed: 'failed'
	};
	const words: Record<TerminalStepState, string> = {
		pending: 'Pending',
		'in-progress': 'In progress',
		done: 'Done',
		failed: 'Failed'
	};

	const items = $derived<StepItem[]>(
		buildSteps(progress).map((step) => ({
			id: step.id,
			status: status[step.state],
			label: step.label,
			stateText: words[step.state],
			message: step.message || undefined,
			attrs: { 'data-state': step.state }
		}))
	);
</script>

<StepList {items} label="Connection progress" />
