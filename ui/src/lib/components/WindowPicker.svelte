<script lang="ts">
	import Button from '#lib/components/form/Button.svelte';
	import { parseWindow, PRESETS, windowToParams, type MetricsWindow } from '#lib/metrics-window.ts';

	/**
	 * Preset buttons plus a custom range. `onchange` gets the URL params of the chosen window; an
	 * invalid custom range shows the reason and changes nothing.
	 */
	let { value, onchange }: { value: MetricsWindow; onchange: (params: URLSearchParams) => void } =
		$props();

	// The inputs follow the window (a preset click clears them) until the user types.
	const initial = $derived(windowToParams(value));
	let from = $derived(initial.get('from') ?? '');
	let to = $derived(initial.get('to') ?? '');
	let error = $state<string | null>(null);

	function applyCustom(e: SubmitEvent) {
		e.preventDefault();
		const parsed = parseWindow(new URLSearchParams({ from, to }), new Date());
		error = parsed.ok ? null : parsed.error.message;
		if (parsed.ok) onchange(windowToParams(parsed.window));
	}
</script>

<div class="ez-window-picker">
	<div class="ez-cluster" role="group" aria-label="Time window">
		{#each PRESETS as p (p)}
			<Button
				variant={value.preset === p ? 'primary' : 'secondary'}
				size="sm"
				aria-pressed={value.preset === p}
				onclick={() => {
					error = null;
					onchange(new URLSearchParams({ window: p }));
				}}>{p}</Button
			>
		{/each}
	</div>
	<form class="ez-cluster" onsubmit={applyCustom}>
		<label>From <input type="date" bind:value={from} /></label>
		<label>To <input type="date" bind:value={to} /></label>
		<Button type="submit" size="sm" aria-pressed={value.preset === 'custom'}>Apply</Button>
	</form>
	{#if error}<p class="ez-window-picker__error" role="alert">{error}</p>{/if}
</div>
