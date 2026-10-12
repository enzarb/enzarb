<script lang="ts">
	import Button from './form/Button.svelte';
	import type { WorkAction } from '#lib/work-items.ts';

	let {
		actions,
		pending = null,
		onaction
	}: {
		actions: WorkAction[];
		pending?: WorkAction | null;
		onaction: (action: WorkAction) => void;
	} = $props();

	const labels: Record<WorkAction, string> = {
		approve: 'Approve plan',
		retry: 'Retry',
		cancel: 'Cancel',
		merge: 'Merge (squash)'
	};
</script>

{#if actions.length}
	<div class="ez-cluster">
		{#each actions as a (a)}
			<Button
				variant={a === 'approve' || a === 'merge'
					? 'primary'
					: a === 'cancel'
						? 'danger'
						: 'secondary'}
				disabled={pending !== null}
				onclick={() => onaction(a)}
			>
				{labels[a]}
			</Button>
		{/each}
	</div>
{/if}
