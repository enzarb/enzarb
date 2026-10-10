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
		cancel: 'Cancel'
	};
</script>

{#if actions.length}
	<p class="actions">
		{#each actions as a (a)}
			<Button
				variant={a === 'approve' ? 'primary' : a === 'cancel' ? 'danger' : 'default'}
				disabled={pending !== null}
				onclick={() => onaction(a)}
			>
				{labels[a]}
			</Button>
		{/each}
	</p>
{/if}
