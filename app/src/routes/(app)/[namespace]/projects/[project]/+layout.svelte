<script lang="ts">
	import { getProject } from '$lib/remote/projects.remote';
	import { page } from '$app/stores';

	let { children } = $props();

	function isActive(base: string, href: string) {
		if (href === base) return $page.url.pathname === base;
		return $page.url.pathname.startsWith(href);
	}
</script>

{#await getProject() then project}
	{@const base = `/${$page.params.namespace}/projects/${$page.params.project}`}
	{@const tabs = [
		{ href: base, label: 'Overview' },
		{ href: `${base}/files`, label: 'Files' },
		{ href: `${base}/registry`, label: 'Registry' },
		{ href: `${base}/deployments`, label: 'Deployments' },
		{ href: `${base}/terminal`, label: 'Terminal' }
	]}
	<div class="project-shell">
		<div class="project-header">
			<div>
				<a href="/{$page.params.namespace}/projects" class="back">← Projects</a>
				<h2>{project.spec.displayName}</h2>
			</div>
			<span class="badge {(project.status?.phase ?? 'pending').toLowerCase()}">{project.status?.phase ?? 'Pending'}</span>
		</div>
		<nav class="project-tabs">
			{#each tabs as tab}
				<a href={tab.href} class="tab {isActive(base, tab.href) ? 'active' : ''}">{tab.label}</a>
			{/each}
		</nav>
		<div class="project-content">
			{@render children()}
		</div>
	</div>
{/await}

<style>
	.project-shell { display: flex; flex-direction: column; height: 100%; }
	.project-header { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 1rem; }
	.back { font-size: 12px; color: var(--color-text-muted); display: block; margin-bottom: 0.25rem; }
	.project-tabs { display: flex; gap: 0; border-bottom: 1px solid var(--color-border); margin-bottom: 1.5rem; }
	.tab { padding: 0.5rem 1rem; color: var(--color-text-muted); font-size: 13px; border-bottom: 2px solid transparent; margin-bottom: -1px; }
	.tab:hover { color: var(--color-text); text-decoration: none; }
	.tab.active { color: var(--color-text); border-bottom-color: var(--color-accent); }
	.project-content { flex: 1; }
</style>
