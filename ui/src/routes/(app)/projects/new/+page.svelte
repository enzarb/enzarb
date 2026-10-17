<script lang="ts">
	import { page } from '$app/state';
	import CreatedProjects from '#lib/components/CreatedProjects.svelte';
	import NewProjectForm from '#lib/components/NewProjectForm.svelte';
	import EmptyState from '#lib/components/ui/EmptyState.svelte';
	import Card from '#lib/components/ui/Card.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import RefreshRepositoriesButton from '#lib/components/RefreshRepositoriesButton.svelte';
	import { REPOSITORY_LIST_HELP, type PlanResult } from '#lib/onboarding.ts';
	import { parseRepoResult } from '#lib/repo-names.ts';
	import {
		createProjects,
		listProjects,
		refreshRepositoryChoices,
		repositoryChoices,
		startCreateRepo
	} from '#lib/projects.remote.ts';

	const choices = repositoryChoices();
	const projects = listProjects();

	let results = $state<PlanResult[] | null>(null);

	// The create-repository round trip comes back with `?newRepo=<key>&repoResult=<code>`.
	const repoCode = $derived(parseRepoResult(page.url.searchParams.get('repoResult')));
	const repoStatus = $derived(Number(page.url.searchParams.get('repoStatus')) || undefined);
	// Refresh asks GitHub again (skipping the short display cache) and updates `choices`.
	const refresh = () => refreshRepositoryChoices();
</script>

<svelte:head><title>New project · enzarb</title></svelte:head>
<PageHeader
	title={results ? 'New projects' : 'New project'}
	description={results ? undefined : 'Pick the repositories a sandbox should work on.'}
/>

{#if results}
	<CreatedProjects
		{results}
		load={async () => {
			await projects.refresh();
			return projects.current ?? [];
		}}
	/>
{:else if choices.current}
	{#if choices.current.repositories.length === 0}
		<!-- The layout also shows the install notice (InstallBanner) when the list is empty. -->
		<Card>
			<EmptyState
				title="No repositories are available yet"
				description={`${choices.current.problem?.code === 'unavailable' ? choices.current.problem.message : REPOSITORY_LIST_HELP} Install the GitHub App or ask for write access, then refresh the list.`}
			/>
			<RefreshRepositoriesButton {refresh} />
		</Card>
	{:else}
		<NewProjectForm
			choices={choices.current}
			{refresh}
			startCreateRepo={(input) => startCreateRepo(input)}
			newRepo={page.url.searchParams.get('newRepo')}
			repoResult={repoCode ? { code: repoCode, status: repoStatus } : null}
			create={async (plans, triggers) => {
				const outcome = await createProjects({ plans, triggers });
				// All-failed stays on the form, which shows the reasons next to the user's selections.
				if (outcome.ok && outcome.results.some((r) => r.status === 'created'))
					results = outcome.results;
				return outcome;
			}}
		/>
	{/if}
{:else}
	<Card><Skeleton label="Loading your repositories" rows={4} /></Card>
{/if}
