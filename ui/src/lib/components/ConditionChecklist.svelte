<script lang="ts">
	import { buildChecklist, type StepState } from '#lib/conditions.ts';
	import type { Condition } from '#lib/project-types.ts';
	import StepList, { type StepItem, type StepStatus } from './ui/StepList.svelte';

	/** Boot progress of a Project, one step per Project condition. */
	let { conditions }: { conditions: Condition[] } = $props();

	const status: Record<StepState, StepStatus> = {
		pending: 'pending',
		'in-progress': 'active',
		done: 'done',
		failed: 'failed'
	};
	const words: Record<StepState, string> = {
		pending: 'Pending',
		'in-progress': 'In progress',
		done: 'Done',
		failed: 'Failed'
	};

	const items = $derived<StepItem[]>(
		buildChecklist(conditions).map((step) => ({
			id: step.type,
			status: status[step.state],
			label: step.label,
			stateText: words[step.state],
			message: step.message || undefined,
			attrs: { 'data-state': step.state }
		}))
	);
</script>

<StepList {items} label="Boot progress" />
