<script lang="ts">
	import Button from './form/Button.svelte';
	import ConfirmDialog from './ui/ConfirmDialog.svelte';

	/**
	 * Restarts the sandbox now instead of waiting for the drain. Bound to the generation the user
	 * sees: if the change moved on, the server refuses and the button offers a retry. After a
	 * successful request it stays disabled until the pending change clears or its generation moves.
	 */
	let { generation, roll }: { generation: number; roll: (generation: number) => Promise<void> } =
		$props();

	let open = $state(false);
	let requested = $state<number | null>(null);
	let failed = $state(false);

	async function confirm() {
		failed = false;
		try {
			await roll(generation);
			requested = generation;
		} catch {
			failed = true;
		}
	}
</script>

<Button variant="danger" disabled={requested === generation} onclick={() => (open = true)}>
	{requested === generation ? 'Restart requested' : 'Roll now'}
</Button>
{#if requested === generation}
	<span class="ez-muted" role="status">The sandbox restarts within a minute.</span>
{:else if failed}
	<span class="ez-muted" role="status">Could not roll now; the change may have moved on.</span>
{/if}
<ConfirmDialog
	bind:open
	tone="danger"
	title="Restart the sandbox now?"
	description="Active sessions and running jobs are interrupted and the sandbox restarts to apply the queued change. Running steps resume after the restart."
	confirmLabel="Roll now"
	onconfirm={confirm}
/>
