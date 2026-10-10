<script lang="ts">
	import type { Snippet } from 'svelte';
	import Icon, { type IconName } from './Icon.svelte';

	/** What to show instead of an empty list: why it is empty and, if possible, what to do next. */
	let {
		title,
		description,
		icon = 'inbox',
		actions,
		level = 2
	}: {
		title: string;
		description?: string;
		icon?: IconName;
		actions?: Snippet;
		/** Heading level of the title; pick the one under the surrounding heading. */
		level?: 2 | 3 | 4;
	} = $props();
</script>

<div class="ez-empty">
	<span class="ez-empty__icon"><Icon name={icon} /></span>
	<svelte:element this={`h${level}`} class="ez-empty__title">{title}</svelte:element>
	{#if description}<p class="ez-empty__description">{description}</p>{/if}
	{#if actions}<div class="ez-empty__actions ez-cluster">{@render actions()}</div>{/if}
</div>
