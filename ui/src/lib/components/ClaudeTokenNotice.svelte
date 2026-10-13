<script lang="ts">
	import LinkButton from './ui/LinkButton.svelte';
	import Notice from './ui/Notice.svelte';
</script>

<Notice tone="info" title="Set up your Claude token">
	Chat runs Claude with your own token. Set it up once in your settings; every Project uses it.
	{#snippet actions()}
		<LinkButton href="/settings#claude">Set up your Claude token</LinkButton>
	{/snippet}
</Notice>
