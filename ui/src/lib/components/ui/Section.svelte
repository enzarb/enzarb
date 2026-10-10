<script lang="ts">
	import type { Snippet } from 'svelte';
	import Card from './Card.svelte';

	/**
	 * A titled block of a page: heading, optional description and actions, then the body. By default
	 * the block sits in a Card; pass `card={false}` for a bare section, `flush` when the body (a
	 * table) should touch the card edges.
	 */
	const uid = $props.id();
	let {
		title,
		description,
		actions,
		card = true,
		flush = false,
		children
	}: {
		title: string;
		description?: string;
		actions?: Snippet;
		card?: boolean;
		flush?: boolean;
		children: Snippet;
	} = $props();
</script>

{#snippet body()}
	<section class="ez-section" class:ez-section--flush={card && flush} aria-labelledby="{uid}-title">
		<div class="ez-section__header">
			<div>
				<h2 class="ez-section__title" id="{uid}-title">{title}</h2>
				{#if description}<p class="ez-section__description">{description}</p>{/if}
			</div>
			{#if actions}<div class="ez-cluster">{@render actions()}</div>{/if}
		</div>
		{@render children()}
	</section>
{/snippet}

{#if card}
	<Card {flush}>{@render body()}</Card>
{:else}
	{@render body()}
{/if}
