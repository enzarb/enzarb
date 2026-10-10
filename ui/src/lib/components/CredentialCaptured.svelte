<script lang="ts">
	import LinkButton from './ui/LinkButton.svelte';
	import Notice from './ui/Notice.svelte';

	/** Shown once the sandbox captured the Claude token; `saveError` is a separate failure to record it. */
	let { name, saveError = '' }: { name: string; saveError?: string } = $props();
</script>

<Notice tone="success" title="Claude token configured">
	The sandbox stored the token. You can close the terminal.
	{#snippet actions()}
		<LinkButton href="/projects/{name}">Back to the Project</LinkButton>
	{/snippet}
</Notice>
{#if saveError}<Notice tone="error">{saveError}</Notice>{/if}
