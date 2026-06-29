<script lang="ts">
	import { getProject, restartWorkspace } from '$lib/remote/projects.remote';
	import { page } from '$app/state';

	let { children } = $props();

	function isActive(base: string, href: string) {
		if (href === base) return page.url.pathname === base;
		return page.url.pathname.startsWith(href);
	}

	// Dismissed version is stored in localStorage so the banner doesn't reappear
	// for the same pending image within this browser session.
	function dismissedKey(slug: string) { return `enzarb-update-dismissed:${slug}`; }
	function isDismissed(slug: string, desiredImage: string) {
		try { return localStorage.getItem(dismissedKey(slug)) === desiredImage; } catch { return false; }
	}
	function dismiss(slug: string, desiredImage: string) {
		try { localStorage.setItem(dismissedKey(slug), desiredImage); } catch {}
		dismissed = true;
	}

	let dismissed = $state(false);
	let restarting = $state(false);
	let restartError = $state('');

	async function handleRestart(slug: string, desiredImage: string) {
		restarting = true;
		restartError = '';
		try {
			await restartWorkspace({ slug });
			dismiss(slug, desiredImage);
			await getProject().refresh();
		} catch (e) {
			restartError = e instanceof Error ? e.message : 'Failed to request restart';
		} finally {
			restarting = false;
		}
	}
</script>

{#await getProject(page.params.project) then project}
	{@const base = `/${page.params.namespace}/projects/${page.params.project}`}
	{@const tabs = [
		{ href: base, label: 'Overview' },
		{ href: `${base}/files`, label: 'Files' },
		{ href: `${base}/registry`, label: 'Registry' },
		{ href: `${base}/terminal`, label: 'Terminal' },
		{ href: `${base}/settings`, label: 'Settings' }
	]}
	<div class="project-shell">
		<div class="project-header">
			<div>
				<a href="/{page.params.namespace}/projects" class="back">← Projects</a>
				<h2>{project.spec.displayName}</h2>
			</div>
			<span class="badge {(project.status?.phase ?? 'pending').toLowerCase()}">{project.status?.phase ?? 'Pending'}</span>
		</div>
		<nav class="project-tabs">
			{#each tabs as tab}
				<a href={tab.href} class="tab {isActive(base, tab.href) ? 'active' : ''}">{tab.label}</a>
			{/each}
		</nav>
		{#each [(project.status?.conditions ?? []).find((c: any) => c.type === 'WorkspaceUpdatePending' && c.status === 'True')].filter(Boolean) as updateCond}
			{#if !dismissed && !isDismissed(project.metadata.name, project.status.desiredWorkspaceImage)}
			<div class="update-banner">
				<div class="update-banner-body">
					<div class="update-banner-title">Workspace update available</div>
					{#if updateCond.message}
						<div class="update-banner-changelog">{updateCond.message}</div>
					{/if}
					{#if restartError}<div class="update-banner-error">{restartError}</div>{/if}
					<div class="update-banner-meta">
						Running: <code>{project.status.runningWorkspaceImage}</code>
						→ Latest: <code>{project.status.desiredWorkspaceImage}</code>
					</div>
				</div>
				<div class="update-banner-actions">
					<button
						class="btn btn-primary btn-sm"
						disabled={restarting}
						onclick={() => handleRestart(project.metadata.name, project.status.desiredWorkspaceImage)}
					>
						{restarting ? 'Requesting…' : 'Restart now'}
					</button>
					<button
						class="btn btn-sm"
						onclick={() => dismiss(project.metadata.name, project.status.desiredWorkspaceImage)}
					>
						Dismiss
					</button>
				</div>
			</div>
			{/if}
		{/each}
		<div class="project-content">
			{@render children()}
		</div>
	</div>
{/await}

<style>
	.project-shell { display: flex; flex-direction: column; height: 100%; }
	.project-header { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 1rem; }
	.back { font-size: 12px; color: var(--color-text-muted); display: block; margin-bottom: 0.25rem; }
	.project-tabs { display: flex; gap: 0; border-bottom: 1px solid var(--color-border); margin-bottom: 1.5rem; overflow-x: auto; overflow-y: hidden; -webkit-overflow-scrolling: touch; }
	.tab { padding: 0.5rem 1rem; color: var(--color-text-muted); font-size: 13px; border-bottom: 2px solid transparent; margin-bottom: -1px; white-space: nowrap; }
	.tab:hover { color: var(--color-text); text-decoration: none; }
	.tab.active { color: var(--color-text); border-bottom-color: var(--color-accent); }
	.project-content { flex: 1; }
	.update-banner { display: flex; align-items: flex-start; justify-content: space-between; gap: 1rem; padding: 0.75rem 1rem; margin-bottom: 1rem; background: #1a1a00; border: 1px solid #5a5a00; border-radius: 6px; }
	.update-banner-body { flex: 1; min-width: 0; }
	.update-banner-title { font-size: 13px; font-weight: 600; color: #e8d44d; margin-bottom: 0.25rem; }
	.update-banner-changelog { font-size: 12px; color: var(--color-text-muted); white-space: pre-wrap; margin-bottom: 0.5rem; max-height: 120px; overflow-y: auto; }
	.update-banner-meta { font-size: 11px; color: var(--color-text-muted); }
	.update-banner-meta code { font-family: var(--font-mono); font-size: 11px; }
	.update-banner-error { font-size: 12px; color: var(--color-danger); margin-bottom: 0.25rem; }
	.update-banner-actions { display: flex; flex-direction: column; gap: 0.4rem; flex-shrink: 0; }
	.btn-sm { padding: 0.3rem 0.7rem; font-size: 12px; }
</style>
