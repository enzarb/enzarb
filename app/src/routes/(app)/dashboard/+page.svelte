<script lang="ts">
	import { createOrg } from '$lib/remote/orgs.remote';
	import type { LayoutData } from '../$types';

	let { data }: { data: LayoutData } = $props();
	let showNewOrg = $state(false);

	const allProjects = $derived(
		data.session.orgs.flatMap((org) =>
			(data.orgProjects[org.slug] ?? []).map((p: { slug: string; displayName: string }) => ({ ...p, orgSlug: org.slug }))
		)
	);
</script>

{#if allProjects.length === 0 && data.session.orgs.length === 0}
	<div class="empty-state">
		<p>You're not a member of any organization yet.</p>
		<button class="btn btn-primary" onclick={() => (showNewOrg = true)}>Create your first organization</button>
	</div>
{:else}
	<div class="page-header">
		<h2>Projects</h2>
		{#if data.session.orgs.length === 1}
			<a href="/{data.session.orgs[0].slug}/projects/new" class="btn btn-primary">New project</a>
		{/if}
	</div>
	{#if allProjects.length === 0}
		<p class="empty">No projects yet. <a href="/{data.session.orgs[0]?.slug}/projects/new">Create one</a>.</p>
	{:else}
		<div class="projects">
			{#each allProjects as project}
				<a href="/{project.orgSlug}/projects/{project.slug}" class="card project-card">
					<div class="project-name">{project.displayName}</div>
					{#if data.session.orgs.length > 1}
						<div class="project-org">{project.orgSlug}</div>
					{/if}
				</a>
			{/each}
		</div>
	{/if}
{/if}

{#if showNewOrg}
	<div class="card new-org-form">
		<h3>Create organization</h3>
		<form {...createOrg}>
			<div class="field">
				<label for="org-name">Display name</label>
				<input id="org-name" {...createOrg.fields.displayName.as('text')} placeholder="Acme Corp" required />
				{#each createOrg.fields.displayName.issues() as issue}<p class="field-error">{issue.message}</p>{/each}
			</div>
			<div class="field">
				<label for="org-slug">Slug</label>
				<input id="org-slug" {...createOrg.fields.slug.as('text')} placeholder="acme-corp" pattern="[a-z0-9][a-z0-9\-]*[a-z0-9]" minlength="2" maxlength="63" required />
				<p class="hint">Lowercase letters, numbers, hyphens. Used in URLs.</p>
				{#each createOrg.fields.slug.issues() as issue}<p class="field-error">{issue.message}</p>{/each}
			</div>
			<button type="submit" class="btn btn-primary">Create</button>
		</form>
	</div>
{/if}

<style>
	.page-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 1.5rem; }
	.projects { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 1rem; }
	.project-card { color: var(--color-text); text-decoration: none; display: block; }
	.project-card:hover { border-color: var(--color-accent); text-decoration: none; }
	.project-name { font-weight: 600; }
	.project-org { font-size: 11px; color: var(--color-text-muted); margin-top: 0.25rem; font-family: var(--font-mono); }
	.empty { color: var(--color-text-muted); }
	.empty-state { text-align: center; padding: 3rem 1rem; color: var(--color-text-muted); }
	.empty-state p { margin: 0 0 1rem; }
	.new-org-form { margin-top: 1.5rem; max-width: 480px; }
	.new-org-form h3 { margin: 0 0 1.25rem; }
	.field { margin-bottom: 1rem; }
	.field label { display: block; font-weight: 500; font-size: 13px; margin-bottom: 0.375rem; }
	.hint { font-size: 12px; color: var(--color-text-muted); margin: 0.25rem 0 0; }
	.field-error { font-size: 13px; color: var(--color-danger); margin: 0.25rem 0 0; }
</style>
