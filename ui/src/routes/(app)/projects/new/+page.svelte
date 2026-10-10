<script lang="ts">
	import { page } from '$app/state';
	import CreatedProjects from '#lib/components/CreatedProjects.svelte';
	import NewProjectForm from '#lib/components/NewProjectForm.svelte';
	import PageHeading from '#lib/components/PageHeading.svelte';
	import type { PlanResult } from '#lib/onboarding.ts';
	import { createProjects, listProjects, repositoryChoices } from '#lib/projects.remote.ts';

	const choices = repositoryChoices();
	const projects = listProjects();
	let results = $state<PlanResult[] | null>(null);

	// The refresh flow re-reads the user's installations and comes back here.
	const refreshHref = $derived(
		`/login/github?refresh=1&return=${encodeURIComponent(page.url.pathname)}`
	);
</script>

<svelte:head><title>New project · enzarb</title></svelte:head>

<PageHeading title={results ? 'New projects' : 'New project'} />

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
		<!-- The layout shows the install notice (InstallBanner) when the snapshot is empty. -->
		<p class="muted">No repositories are available yet.</p>
	{:else}
		<NewProjectForm
			choices={choices.current}
			{refreshHref}
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
	<p class="muted">Loading your repositories…</p>
{/if}
