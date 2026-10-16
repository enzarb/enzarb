<script lang="ts">
	import Button from './form/Button.svelte';

	/** Re-reads the repository list from GitHub, skipping the short display cache. */
	let {
		refresh,
		variant = 'secondary'
	}: { refresh: () => Promise<void>; variant?: 'primary' | 'secondary' | 'ghost' } = $props();

	let pending = $state(false);
	let failed = $state(false);

	async function run() {
		pending = true;
		failed = false;
		try {
			await refresh();
		} catch {
			failed = true;
		} finally {
			pending = false;
		}
	}
</script>

<Button {variant} loading={pending} onclick={run}>Refresh repositories</Button>
{#if failed}<span class="ez-muted" role="status">Could not refresh. Try again.</span>{/if}
