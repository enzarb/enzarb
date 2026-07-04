<script lang="ts">
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import TilingShell from '$lib/tiling/TilingShell.svelte';

	const base = $derived(`/${page.params.namespace}/projects/${page.params.project}`);
	let narrow = $state(false);

	onMount(() => {
		narrow = window.innerWidth < 768;
		const handler = () => { narrow = window.innerWidth < 768; };
		window.addEventListener('resize', handler);
		return () => window.removeEventListener('resize', handler);
	});
</script>

{#if narrow}
	<div class="mobile-message">
		<p>Tiling mode is designed for desktop browsers.</p>
		<a href={base}>← Back to standard view</a>
	</div>
{:else}
	<TilingShell namespace={page.params.namespace!} project={page.params.project!} />
{/if}

<style>
	.mobile-message { display: flex; flex-direction: column; align-items: center; justify-content: center; height: 100%; gap: 1rem; padding: 2rem; text-align: center; }
	.mobile-message p { font-size: 15px; color: var(--color-text-muted); }
	.mobile-message a { font-size: 13px; color: var(--color-accent); }
</style>
