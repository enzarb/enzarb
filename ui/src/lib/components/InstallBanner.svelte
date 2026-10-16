<script lang="ts">
	import { REPOSITORY_LIST_HELP } from '#lib/onboarding.ts';
	import RefreshRepositoriesButton from './RefreshRepositoriesButton.svelte';
	import LinkButton from './ui/LinkButton.svelte';
	import Notice from './ui/Notice.svelte';

	/** Shown when the live repository list holds nothing for the user (display only). */
	let { installUrl, refresh }: { installUrl: string; refresh: () => Promise<void> } = $props();
</script>

<Notice tone="info" title="Install the GitHub App">
	No repositories are available to you. {REPOSITORY_LIST_HELP} Install the enzarb GitHub App on your account
	or organization, or ask for write access, then refresh the list.
	{#snippet actions()}
		<LinkButton variant="primary" href={installUrl} target="_blank" rel="noopener noreferrer">
			Install the GitHub App<span class="ez-sr-only"> (opens in a new tab)</span>
		</LinkButton>
		<RefreshRepositoriesButton {refresh} />
	{/snippet}
</Notice>
