<script lang="ts">
	import {
		getProjects,
		getDeletedProjects,
		recoverProjectCommand
	} from '$lib/remote/projects.remote';
	import { page } from '$app/stores';

	let recovering = $state('');

	async function handleRecover(slug: string) {
		recovering = slug;
		try {
			await recoverProjectCommand({ slug });
			await getProjects().refresh();
			await getDeletedProjects().refresh();
		} finally {
			recovering = '';
		}
	}
</script>

<div class="page-header">
	<h2>Projects</h2>
	<a href="/{$page.params.namespace}/projects/new" class="btn btn-primary">New project</a>
</div>

<div class="projects">
	{#each await getProjects() as project}
		{@const status = project.status?.phase ?? 'Pending'}
		<a href="/{$page.params.namespace}/projects/{project.metadata.name}" class="card project-card">
			<div class="project-header">
				<span class="project-name">{project.spec.displayName}</span>
				<span class="badge {status.toLowerCase()}">{status}</span>
			</div>
			<div class="project-slug">{project.metadata.name}</div>
			{#if project.spec.tools?.length}
				<div class="tools">
					{#each project.spec.tools as tool}
						<span class="badge">{tool.name}</span>
					{/each}
				</div>
			{/if}
		</a>
	{:else}
		<p class="empty">No projects yet. <a href="/{$page.params.namespace}/projects/new">Create one</a>.</p>
	{/each}
</div>

{#await getDeletedProjects() then deleted}
	{#if deleted.length}
		<div class="deleted-section">
			<h3>Pending deletion</h3>
			<p class="empty">Recoverable until their purge time, after which they're permanently removed.</p>
			<div class="deleted-list">
				{#each deleted as project}
					<div class="card deleted-row">
						<div>
							<span class="project-name">{project.slug}</span>
							<span class="project-slug">purges {new Date(project.purgeAfter).toLocaleString()}</span>
						</div>
						<button
							class="btn"
							disabled={recovering === project.slug}
							onclick={() => handleRecover(project.slug)}
						>
							{recovering === project.slug ? 'Recovering…' : 'Recover'}
						</button>
					</div>
				{/each}
			</div>
		</div>
	{/if}
{/await}

<style>
	.page-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 1.5rem; }
	.projects { display: grid; grid-template-columns: repeat(auto-fill, minmax(280px, 1fr)); gap: 1rem; }
	.project-card { color: var(--color-text); text-decoration: none; display: block; }
	.project-card:hover { border-color: var(--color-accent); text-decoration: none; }
	.project-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.25rem; }
	.project-name { font-weight: 600; }
	.project-slug { color: var(--color-text-muted); font-family: var(--font-mono); font-size: 12px; margin-bottom: 0.5rem; }
	.tools { display: flex; flex-wrap: wrap; gap: 0.25rem; }
	.empty { color: var(--color-text-muted); }
	.deleted-section { margin-top: 2.5rem; }
	.deleted-section h3 { font-size: 14px; margin-bottom: 0.25rem; }
	.deleted-list { display: flex; flex-direction: column; gap: 0.5rem; margin-top: 1rem; }
	.deleted-row { display: flex; justify-content: space-between; align-items: center; gap: 1rem; }
</style>
