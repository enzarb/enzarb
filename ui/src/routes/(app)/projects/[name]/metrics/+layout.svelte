<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import WindowPicker from '#lib/components/WindowPicker.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import { resolveWindow } from '#lib/metrics-window.ts';

	let { children } = $props();

	// Presets are anchored to the moment the URL last changed, so a reload or a shared link is stable.
	const resolved = $derived(resolveWindow(page.url.searchParams, new Date()));
	const parsed = $derived(resolved.parsed);

	function change(params: URLSearchParams) {
		void goto(`?${params}`, { replace: true, reset: false });
	}
</script>

<svelte:head><title>Metrics · {page.params.name} · enzarb</title></svelte:head>

<div class="ez-stack">
	{#if !parsed.ok}
		<Notice tone="warning" title="Showing the last 7 days">{parsed.error.message}</Notice>
	{/if}
	<WindowPicker value={resolved.current} onchange={change} />
	{@render children()}
</div>
