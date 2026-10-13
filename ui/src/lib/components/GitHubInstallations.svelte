<script lang="ts">
	import KeyValueList from './ui/KeyValueList.svelte';
	import LinkButton from './ui/LinkButton.svelte';
	import Notice from './ui/Notice.svelte';

	/** GitHub App installations reachable by the user, with the install and refresh actions. */
	let {
		installUrl,
		refreshHref,
		installations,
		fetchedAt
	}: {
		installUrl: string;
		refreshHref: string;
		installations: { accountLogin: string; installationId: number; repositoryCount: number }[];
		/** ISO time of the snapshot, or null when there is none. */
		fetchedAt: string | null;
	} = $props();

	const describe = (i: { installationId: number; repositoryCount: number }) =>
		`${i.repositoryCount} ${i.repositoryCount === 1 ? 'repository' : 'repositories'} (installation ${i.installationId})`;
</script>

<section class="ez-stack" aria-labelledby="gh-installations">
	<h2 id="gh-installations">GitHub App installations</h2>
	<KeyValueList items={[{ term: 'Repositories last refreshed', value: fetchedAt ?? 'Never' }]} />
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
		Install or configure the GitHub App on GitHub, then choose "Refresh repositories". New
		repositories only appear here after a refresh. The refreshed list also bounds which repositories
		can go into a new Project (for 24 hours). Organizations with OAuth-app restrictions may not
		appear in this list, so not every installation is necessarily shown.
		{#snippet actions()}
			<LinkButton variant="primary" href={installUrl} target="_blank" rel="noopener noreferrer">
				Install / configure the GitHub App<span class="ez-sr-only"> (opens in a new tab)</span>
			</LinkButton>
			<LinkButton href={refreshHref} data-sveltekit-reload>Refresh repositories</LinkButton>
		{/snippet}
	</Notice>
</section>
