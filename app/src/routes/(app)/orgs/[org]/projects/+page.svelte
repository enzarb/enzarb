<script lang="ts">
	import type { PageData } from './$types';
	let { data }: { data: PageData } = $props();
</script>

<div class="page-header">
	<h2>Projects</h2>
	<a href="/orgs/{data.org.id}/projects/new" class="btn btn-primary">New project</a>
</div>

<div class="projects">
	{#each data.projects as project}
		{@const status = project.status?.phase ?? 'Pending'}
		<a href="/orgs/{data.org.id}/projects/{project.metadata.name}" class="card project-card">
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
		<p class="empty">No projects yet. <a href="/orgs/{data.org.id}/projects/new">Create one</a>.</p>
	{/each}
</div>

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
</style>
