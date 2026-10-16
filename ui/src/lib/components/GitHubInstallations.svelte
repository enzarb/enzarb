<script lang="ts">
	import { REPOSITORY_LIST_HELP } from '#lib/onboarding.ts';
	import RefreshRepositoriesButton from './RefreshRepositoriesButton.svelte';
	import KeyValueList from './ui/KeyValueList.svelte';
	import LinkButton from './ui/LinkButton.svelte';
	import Notice from './ui/Notice.svelte';

	/** GitHub App installations holding repositories the user can write to, with install and refresh. */
	let {
		installUrl,
		refresh,
		installations,
		fetchedAt
	}: {
		installUrl: string;
		refresh: () => Promise<void>;
		installations: { accountLogin: string; installationId: number; repositoryCount: number }[];
		/** ISO time GitHub was last asked, or null when it could not be. */
		fetchedAt: string | null;
	} = $props();

	const describe = (i: { installationId: number; repositoryCount: number }) =>
		`${i.repositoryCount} ${i.repositoryCount === 1 ? 'repository' : 'repositories'} (installation ${i.installationId})`;
</script>

<section class="ez-stack" aria-labelledby="gh-installations">
	<h2 id="gh-installations">GitHub App installations</h2>
	<KeyValueList items={[{ term: 'Repositories last read', value: fetchedAt ?? 'Never' }]} />
	{#if installations.length === 0}
		<p>No installations found. Install the GitHub App on an account or organization.</p>
	{:else}
		<ul>
			{#each installations as inst (`${inst.accountLogin}/${inst.installationId}`)}
				<li>
					<strong>{inst.accountLogin}</strong>: <span>{describe(inst)}</span>
				</li>
			{/each}
		</ul>
	{/if}
	<Notice tone="info" title="Adding organizations or repositories">
		{REPOSITORY_LIST_HELP} The list is read live from GitHub (kept for about a minute); install or configure
		the GitHub App on GitHub, then choose "Refresh repositories" to see a change right away.
		{#snippet actions()}
			<LinkButton variant="primary" href={installUrl} target="_blank" rel="noopener noreferrer">
				Install / configure the GitHub App<span class="ez-sr-only"> (opens in a new tab)</span>
			</LinkButton>
			<RefreshRepositoriesButton {refresh} />
		{/snippet}
	</Notice>
</section>
