<script lang="ts">
	import { onMount } from 'svelte';
	import { theme } from '#lib/theme.svelte.ts';
	import type { ThemeChoice } from '#lib/theme.ts';
	import Icon, { type IconName } from './Icon.svelte';

	/**
	 * System / Light / Dark. The choice is remembered in localStorage and applied to <html> as
	 * `data-theme` (the inline script in app.html re-applies it before first paint on the next
	 * visit). A radio group: arrow keys move and select.
	 */
	const options: { value: ThemeChoice; label: string; icon: IconName }[] = [
		{ value: 'system', label: 'System', icon: 'monitor' },
		{ value: 'light', label: 'Light', icon: 'sun' },
		{ value: 'dark', label: 'Dark', icon: 'moon' }
	];

	onMount(() => theme.sync());

	function onkeydown(e: KeyboardEvent) {
		const step =
			e.key === 'ArrowRight' || e.key === 'ArrowDown'
				? 1
				: e.key === 'ArrowLeft' || e.key === 'ArrowUp'
					? -1
					: 0;
		if (!step) return;
		e.preventDefault();
		const i = options.findIndex((o) => o.value === theme.choice);
		const next = options[(i + step + options.length) % options.length];
		theme.set(next.value);
		(e.currentTarget as HTMLElement)
			.querySelector<HTMLElement>(`[data-value="${next.value}"]`)
			?.focus();
	}
</script>

<div class="ez-segmented" role="radiogroup" aria-label="Theme" tabindex="-1" {onkeydown}>
	{#each options as o (o.value)}
		<button
			type="button"
			role="radio"
			class="ez-segmented__option"
			aria-checked={theme.choice === o.value}
			tabindex={theme.choice === o.value ? 0 : -1}
			data-value={o.value}
			onclick={() => theme.set(o.value)}
		>
			<Icon name={o.icon} />
			{o.label}
		</button>
	{/each}
</div>
