<script lang="ts">
	import { Tabs } from 'bits-ui';
	import type { Snippet } from 'svelte';

	/**
	 * Tabs on bits-ui: roving focus with ArrowLeft/ArrowRight (Home/End jump), the selected panel
	 * follows focus. `panel` renders the body for the selected tab value.
	 */
	let {
		tabs,
		value = $bindable(''),
		label,
		panel
	}: {
		tabs: { value: string; label: string; disabled?: boolean }[];
		value?: string;
		label: string;
		panel: Snippet<[string]>;
	} = $props();

	// Select the first tab until the owner (or the user) picks one.
	$effect.pre(() => {
		if (!value) value = tabs[0]?.value ?? '';
	});
</script>

<Tabs.Root bind:value class="ez-tabs">
	<Tabs.List class="ez-tabs__list" aria-label={label}>
		{#each tabs as t (t.value)}
			<Tabs.Trigger class="ez-tabs__trigger" value={t.value} disabled={t.disabled}>
				{t.label}
			</Tabs.Trigger>
		{/each}
	</Tabs.List>
	{#each tabs as t (t.value)}
		<Tabs.Content class="ez-tabs__panel" value={t.value}>{@render panel(t.value)}</Tabs.Content>
	{/each}
</Tabs.Root>
