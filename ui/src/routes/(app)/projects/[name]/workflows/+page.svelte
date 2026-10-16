<script lang="ts">
	import { page } from '$app/state';
	import EmptyState from '#lib/components/ui/EmptyState.svelte';
	import LinkButton from '#lib/components/ui/LinkButton.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { projectWorkflows } from '#lib/workflows.remote.ts';

	const name = $derived(page.params.name!);
	const repos = $derived(projectWorkflows(name));
	const edit = (repo: string, params: Record<string, string>) =>
		`/projects/${encodeURIComponent(name)}/workflows/edit?${new URLSearchParams({ repo, ...params })}`;
</script>

<svelte:head><title>Workflows · {name} · enzarb</title></svelte:head>

{#if repos.error && !repos.current}
	<Notice tone="error" title="Could not load workflows"
		>Something went wrong while reading the repositories.</Notice
	>
{:else if !repos.current}
	<Skeleton label="Loading workflows" rows={3} />
{:else if repos.current.length === 0}
	<EmptyState
		title="No GitHub repositories"
		description="Add a repository in Settings to define its workflows."
	/>
{:else}
	<div class="ez-stack">
		{#each repos.current as r (r.repo)}
			<Section
				title={r.repo}
				description={r.defaultBranch ? `.enzarb/workflows on ${r.defaultBranch}` : undefined}
			>
				{#if r.error}
					<Notice tone="error">{r.error}</Notice>
				{:else if !r.workflows?.length}
					<p class="ez-muted">No definition: uses the built-in workflow.</p>
					<LinkButton href={edit(r.repo, { example: 'issue' })}>Start from example</LinkButton>
				{:else}
					<ul class="wf-list">
						{#each r.workflows as path (path)}
							<li>
								<a href={edit(r.repo, { path })}>{path.slice('.enzarb/workflows/'.length)}</a>
							</li>
						{/each}
					</ul>
					<LinkButton size="sm" href={edit(r.repo, { example: 'issue' })}
						>New from example</LinkButton
					>
				{/if}
				{#if r.prompts?.length}
					<p class="ez-muted">Prompt files: {r.prompts.join(', ')}</p>
				{/if}
			</Section>
		{/each}
	</div>
{/if}

<style>
	.wf-list {
		margin: 0 0 var(--ez-space-3);
		padding-left: var(--ez-space-4);
		font-family: monospace;
	}
</style>
