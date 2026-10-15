<script lang="ts" module>
	import type { StepStatus } from '../ui/StepList.svelte';

	export type ProposeStep = {
		id: 'branch' | 'commit' | 'pr';
		status: StepStatus;
		message?: string;
	};

	export const PROPOSE_LABELS: Record<ProposeStep['id'], string> = {
		branch: 'Create branch',
		commit: 'Commit changes',
		pr: 'Open pull request'
	};
</script>

<script lang="ts">
	import StepList, { type StepItem } from '../ui/StepList.svelte';

	/** Progress of "propose change": branch, commit, pull request. */
	let { steps, url }: { steps: ProposeStep[]; url?: string } = $props();

	const words: Record<StepStatus, string> = {
		pending: 'Pending',
		active: 'In progress',
		done: 'Done',
		failed: 'Failed'
	};
	const items = $derived<StepItem[]>(
		steps.map((s) => ({
			id: s.id,
			status: s.status,
			label: PROPOSE_LABELS[s.id],
			stateText: words[s.status],
			message: s.message,
			attrs: { 'data-step': s.id }
		}))
	);
</script>

<StepList {items} label="Proposal progress" />
{#if url}<p>
		<a href={url} target="_blank" rel="noopener noreferrer">View the pull request</a>
	</p>{/if}
