<script lang="ts">
	import { waitLinks, type Wait } from '#lib/work-items.ts';

	/** What a change request is waiting on, with the links that help (review, checks, PR, comment). */
	let { wait, url }: { wait: Wait | null | undefined; url: string | null } = $props();
	const links = $derived(waitLinks(wait ?? null, url));
</script>

{#if wait?.text}
	<span class="ez-wait ez-wait--{wait.waitingOn}">
		{wait.text}
		{#each links as l (l.href)}
			· <a href={l.href} rel="noopener noreferrer">{l.label}</a>
		{/each}
	</span>
{/if}
