<script lang="ts">
	import LinkButton from './ui/LinkButton.svelte';
	import Notice from './ui/Notice.svelte';

	/** Shown when the user can reach no repository through an installation of the GitHub App. */
	// The snapshot behind this banner also bounds which repositories may be put in a new Project
	// (24 hours, see project-onboarding.ts); it is not used for any other decision.
	let { installUrl, refreshHref }: { installUrl: string; refreshHref: string } = $props();
</script>

<Notice tone="info" title="Install the GitHub App">
	Install the enzarb GitHub App on your account or organization to use your repositories, then
	refresh the list.
	{#snippet actions()}
		<LinkButton variant="primary" href={installUrl} target="_blank" rel="noopener noreferrer">
			Install the GitHub App<span class="ez-sr-only"> (opens in a new tab)</span>
		</LinkButton>
		<LinkButton href={refreshHref} data-sveltekit-reload>Refresh repositories</LinkButton>
	{/snippet}
</Notice>
