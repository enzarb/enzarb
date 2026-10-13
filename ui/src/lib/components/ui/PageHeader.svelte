<script lang="ts">
	import type { Snippet } from 'svelte';

	/** Page title (the only <h1>), optional description line and a right-aligned actions slot. */
	let {
		title,
		description,
		actions,
		meta
	}: {
		title: string;
		description?: string;
		actions?: Snippet;
		/** Free-form line under the title (badges, identifiers). */
		meta?: Snippet;
	} = $props();
</script>

<header class="ez-page-header">
	<div class="ez-page-header__text">
		<h1>{title}</h1>
		{#if meta}<div class="ez-cluster ez-page-header__description">{@render meta()}</div>{/if}
		{#if description}<p class="ez-page-header__description">{description}</p>{/if}
	</div>
	{#if actions}<div class="ez-page-header__actions">{@render actions()}</div>{/if}
</header>
