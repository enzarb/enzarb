<script lang="ts">
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
		cancel: 'Cancel'
	};
</script>

{#if actions.length}
	<p class="actions">
		{#each actions as a (a)}
			<button
				class:primary={a === 'approve'}
				class:danger={a === 'cancel'}
				disabled={pending !== null}
				onclick={() => onaction(a)}
			>
				{labels[a]}
			</button>
		{/each}
	</p>
{/if}
