<script lang="ts">
	import {
		getProjects,
		getDeletedProjects,
		recoverProjectCommand
	} from '$lib/remote/projects.remote';
	import { page } from '$app/state';

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

	let menuOpen = $state(false);
	let showSuspended = $state(false);

	function isSuspended(project: { status?: { phase?: string } }) {
		return (project.status?.phase ?? 'Pending') === 'Suspended';
	}
</script>

<svelte:window onclick={() => (menuOpen = false)} />

{#await getProjects() then allProjects}
	{@const suspendedCount = allProjects.filter(isSuspended).length}
	<div class="page-header">
		<h2>Projects</h2>
		<div class="page-header-actions">
			<a href="/{page.params.namespace}/projects/new" class="btn btn-primary">New project</a>
			<div class="dropdown" class:open={menuOpen}>
				<button
					class="btn btn-subtle dropdown-trigger"
					title="Project list options"
					onclick={(e) => { e.stopPropagation(); menuOpen = !menuOpen; }}
				>
					⋯
				</button>
				<div class="dropdown-menu">
					<button class="dropdown-item" onclick={() => { showSuspended = !showSuspended; menuOpen = false; }}>
						<span class="check-mark">{showSuspended ? '✓' : ''}</span>
						Show suspended{suspendedCount ? ` (${suspendedCount})` : ''}
					</button>
				</div>
			</div>
		</div>
	</div>

	<div class="projects">
		{#each allProjects.filter((p: { status?: { phase?: string } }) => showSuspended || !isSuspended(p)) as project}
			{@const status = project.status?.phase ?? 'Pending'}
			<a href="/{page.params.namespace}/projects/{project.metadata.name}" class="card project-card">
				<div class="project-header">
					<span class="project-name">{project.spec.displayName}</span>
					<span class="badge {status.toLowerCase()}">{status}</span>
				</div>
				<div class="project-slug">{project.metadata.name}</div>
			</a>
		{:else}
			<p class="empty">
				{#if allProjects.length}
					All projects are suspended. <button class="link-btn" onclick={() => (showSuspended = true)}>Show suspended</button>.
				{:else}
					No projects yet. <a href="/{page.params.namespace}/projects/new">Create one</a>.
				{/if}
			</p>
		{/each}
	</div>
{/await}

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
	.page-header-actions { display: flex; align-items: center; gap: 0.5rem; }
	.link-btn { background: none; border: none; color: var(--color-text); cursor: pointer; padding: 0; font-size: 13px; text-decoration: underline; }

	/* Overflow dropdown (project list options) */
	.dropdown { position: relative; }
	.dropdown-trigger { padding: 0.4rem 0.65rem; font-size: 15px; line-height: 1; }
	.btn-subtle { background: none; border-color: var(--color-border); color: var(--color-text-muted); }
	.btn-subtle:hover { color: var(--color-text); }
	.dropdown-menu {
		display: none;
		position: absolute;
		right: 0;
		top: calc(100% + 4px);
		min-width: 190px;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius);
		box-shadow: var(--shadow);
		z-index: 10;
		overflow: hidden;
	}
	.dropdown.open .dropdown-menu { display: block; }
	.dropdown-item {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		width: 100%;
		padding: 0.45rem 0.75rem;
		background: none;
		border: none;
		color: var(--color-text);
		font-size: 13px;
		cursor: pointer;
		text-align: left;
		white-space: nowrap;
	}
	.dropdown-item:hover { background: var(--color-surface-2); }
	.check-mark { display: inline-block; width: 1em; text-align: center; font-size: 12px; flex-shrink: 0; }
	.projects { display: grid; grid-template-columns: repeat(auto-fill, minmax(280px, 1fr)); gap: 1rem; }
	.project-card { color: var(--color-text); text-decoration: none; display: block; }
	.project-card:hover { border-color: var(--color-accent); text-decoration: none; }
	.project-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.25rem; }
	.project-name { font-weight: 600; }
	.project-slug { color: var(--color-text-muted); font-family: var(--font-mono); font-size: 12px; }
	.empty { color: var(--color-text-muted); }
	.deleted-section { margin-top: 2.5rem; }
	.deleted-section h3 { font-size: 14px; margin-bottom: 0.25rem; }
	.deleted-list { display: flex; flex-direction: column; gap: 0.5rem; margin-top: 1rem; }
	.deleted-row { display: flex; justify-content: space-between; align-items: center; gap: 1rem; }
</style>
