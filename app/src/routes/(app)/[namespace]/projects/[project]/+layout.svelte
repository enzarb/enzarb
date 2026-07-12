<script lang="ts">
	import { getProject, restartWorkspace } from '$lib/remote/projects.remote';
	import { workspaceHealth } from '$lib/workspaceHealth.svelte';
	import { page } from '$app/state';
	import { toErrorMessage } from '$lib/errors';

	let { children } = $props();

	function isActive(base: string, href: string) {
		if (href === base) return page.url.pathname === base;
		return page.url.pathname.startsWith(href);
	}

	// Dismissed version is stored in localStorage so the banner doesn't reappear
	// for the same pending image within this browser session.
	function dismissedKey(slug: string) { return `enzarb-update-dismissed:${slug}`; }
	function isImageDismissed(slug: string, desiredImage: string) {
		try { return localStorage.getItem(dismissedKey(slug)) === desiredImage; } catch { return false; }
	}
	function dismissImage(slug: string, desiredImage: string) {
		try { localStorage.setItem(dismissedKey(slug), desiredImage); } catch {}
		imageDismissed = true;
	}

	const projectData = $derived(getProject(page.params.project));

	let imageDismissed = $state(false);
	let envDismissed = $state(false);
	let restarting = $state(false);
	let restartError = $state('');

	// Flip the shared health tracker to unhealthy the moment a restart is
	// requested: every tab pauses its connections until /healthz answers again,
	// and the overlay below tells the user why.
	async function markRestarting() {
		try {
			const project = await getProject(page.params.project);
			const agentPath = project?.status?.agentPath;
			if (agentPath) workspaceHealth(`https://enzarb.dev${agentPath}`).markUnhealthy();
		} catch {}
	}

	// A single restart satisfies every pending-restart reason at once, so one
	// button drives all of them and dismisses whichever are currently showing.
	async function handleRestart(project: any, reasons: { kind: 'image' | 'env' }[]) {
		restarting = true;
		restartError = '';
		try {
			await restartWorkspace({ slug: project.metadata.name });
			for (const reason of reasons) {
				if (reason.kind === 'image') dismissImage(project.metadata.name, project.status.desiredWorkspaceImage);
				else if (reason.kind === 'env') envDismissed = true;
			}
			await markRestarting();
			await getProject().refresh();
		} catch (e) {
			restartError = toErrorMessage(e, 'Failed to request restart');
		} finally {
			restarting = false;
		}
	}

	function handleDismiss(project: any, reasons: { kind: 'image' | 'env' }[]) {
		for (const reason of reasons) {
			if (reason.kind === 'image') dismissImage(project.metadata.name, project.status.desiredWorkspaceImage);
			else if (reason.kind === 'env') envDismissed = true;
		}
	}

	// Poll while the project is still provisioning so the "Pending" badge and
	// lock overlay clear as soon as the operator finishes reconciling.
	$effect(() => {
		const slug = page.params.project;
		let cancelled = false;
		const timer = setInterval(async () => {
			if (cancelled) return;
			try {
				const project = await getProject(slug);
				if (project.status?.phase === 'Pending') {
					await getProject(slug).refresh();
				}
			} catch {}
		}, 3000);
		return () => {
			cancelled = true;
			clearInterval(timer);
		};
	});
</script>

{#await projectData then project}
	{@const base = `/${page.params.namespace}/projects/${page.params.project}`}
	{@const health = project.status?.agentPath
		? workspaceHealth(`https://enzarb.dev${project.status.agentPath}`)
		: null}
	{@const tabs = [
		{ href: base, label: 'Overview' },
		{ href: `${base}/files`, label: 'Files' },
		{ href: `${base}/registry`, label: 'Registry' },
		{ href: `${base}/terminal`, label: 'Terminal' },
		{ href: `${base}/agents`, label: 'Agents' },
		{ href: `${base}/settings`, label: 'Settings' },
		{ href: `${base}/billing`, label: 'Billing' },
		{ href: `${base}/utilization`, label: 'Utilization' }
	]}
	{@const isTiling = page.url.pathname.endsWith('/tiling')}
	{@const imageCond = (project.status?.conditions ?? []).find((c: any) => c.type === 'WorkspaceUpdatePending' && c.status === 'True')}
	{@const restartReasons = [
			...(imageCond && !imageDismissed && !isImageDismissed(project.metadata.name, project.status.desiredWorkspaceImage)
				? [{
					kind: 'image' as const,
					title: 'Workspace update available',
					detail: imageCond.message,
					meta: `Running: ${project.status.runningWorkspaceImage} → Latest: ${project.status.desiredWorkspaceImage}`
				}]
				: []),
			...(!envDismissed && project.metadata?.annotations?.['enzarb.io/env-update-pending'] === 'true'
				? [{
					kind: 'env' as const,
					title: 'Environment variables updated',
					detail: 'Your workspace environment variables have changed (e.g. GitHub token or user secrets). Restart to apply the new values.',
					meta: null
				}]
				: [])
		]}
		<div class="project-shell">
			<div class="project-header">
				<div>
					<a href="/{page.params.namespace}/projects" class="back">← Projects</a>
					<h2>{project.spec.displayName}</h2>
				</div>
				<div class="header-right">
					<a href={isTiling ? base : `${base}/tiling`} class="tiling-toggle" title={isTiling ? 'Standard view' : 'Tiling mode'}>
						{isTiling ? '⊞' : '⊟'}
					</a>
					<span class="badge {(project.status?.phase ?? 'pending').toLowerCase()}">{project.status?.phase ?? 'Pending'}</span>
				</div>
			</div>
			<nav class="project-tabs" class:hidden={isTiling}>
				{#each tabs as tab}
					<a href={tab.href} class="tab {isActive(base, tab.href) ? 'active' : ''}">{tab.label}</a>
				{/each}
			</nav>
		{#if restartReasons.length > 0}
			<div class="update-banner">
				<div class="update-banner-body">
					{#each restartReasons as reason}
						<div class="update-banner-title">{reason.title}</div>
						{#if reason.detail}
							<div class="update-banner-changelog">{reason.detail}</div>
						{/if}
						{#if reason.meta}
							<div class="update-banner-meta">{reason.meta}</div>
						{/if}
					{/each}
					{#if restartError}<div class="update-banner-error">{restartError}</div>{/if}
				</div>
				<div class="update-banner-actions">
					<button
						class="btn btn-primary btn-sm"
						disabled={restarting}
						onclick={() => handleRestart(project, restartReasons)}
					>
						{restarting ? 'Requesting…' : 'Restart now'}
					</button>
					<button
						class="btn btn-sm"
						onclick={() => handleDismiss(project, restartReasons)}
					>
						Dismiss
					</button>
				</div>
			</div>
		{/if}
		<div class="project-content-wrap">
			<div class="project-content" class:locked={project.status?.phase === 'Pending' || (project.status?.phase !== 'Suspended' && health?.state === 'unhealthy')} class:tiling={isTiling}>
				{@render children()}
			</div>
			{#if project.status?.phase === 'Pending'}
				<div class="provisioning-overlay">
					<div class="spinner"></div>
					<p>Provisioning workspace…</p>
				</div>
			{:else if project.status?.phase !== 'Suspended' && health?.state === 'unhealthy'}
				<div class="provisioning-overlay">
					<div class="spinner"></div>
					<p>Workspace is restarting — reconnecting when it's back…</p>
				</div>
			{/if}
		</div>
	</div>
{/await}

<style>
	.project-shell { display: flex; flex-direction: column; height: 100%; }
	.project-header { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 1rem; }
	.back { font-size: 12px; color: var(--color-text-muted); display: block; margin-bottom: 0.25rem; }
	.header-right { display: flex; align-items: center; gap: 0.5rem; }
	.tiling-toggle { font-size: 18px; color: var(--color-text-muted); text-decoration: none; line-height: 1; padding: 0.15rem; border-radius: 4px; }
	.tiling-toggle:hover { color: var(--color-accent); }
	.project-tabs { display: flex; gap: 0; border-bottom: 1px solid var(--color-border); margin-bottom: 1.5rem; overflow-x: auto; overflow-y: hidden; -webkit-overflow-scrolling: touch; }
	.project-tabs.hidden { display: none; }
	.tab { padding: 0.5rem 1rem; color: var(--color-text-muted); font-size: 13px; border-bottom: 2px solid transparent; margin-bottom: -1px; white-space: nowrap; }
	.tab:hover { color: var(--color-text); text-decoration: none; }
	.tab.active { color: var(--color-text); border-bottom-color: var(--color-accent); }
	.project-content-wrap { position: relative; flex: 1; display: flex; overflow: hidden; }
	.project-content { flex: 1; overflow-y: auto; }
	.project-content.tiling { overflow: hidden; display: flex; flex-direction: column; padding: 0; }
	.project-content.locked { pointer-events: none; opacity: 0.4; }
	.provisioning-overlay {
		position: absolute;
		inset: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 0.75rem;
		background: rgba(0, 0, 0, 0.25);
		z-index: 5;
	}
	.provisioning-overlay p { font-size: 13px; color: var(--color-text); }
	.spinner {
		width: 28px;
		height: 28px;
		border: 3px solid var(--color-border);
		border-top-color: var(--color-accent);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}
	@keyframes spin {
		to { transform: rotate(360deg); }
	}
	.update-banner { display: flex; align-items: flex-start; justify-content: space-between; gap: 1rem; padding: 0.75rem 1rem; margin-bottom: 1rem; background: #1a1a00; border: 1px solid #5a5a00; border-radius: 6px; }
	.update-banner-body { flex: 1; min-width: 0; }
	.update-banner-title { font-size: 13px; font-weight: 600; color: #e8d44d; margin-bottom: 0.25rem; }
	.update-banner-title:not(:first-child) { margin-top: 0.75rem; }
	.update-banner-changelog { font-size: 12px; color: var(--color-text-muted); white-space: pre-wrap; margin-bottom: 0.5rem; max-height: 120px; overflow-y: auto; }
	.update-banner-meta { font-size: 11px; color: var(--color-text-muted); font-family: var(--font-mono); }
	.update-banner-error { font-size: 12px; color: var(--color-danger); margin-bottom: 0.25rem; }
	.update-banner-actions { display: flex; flex-direction: column; gap: 0.4rem; flex-shrink: 0; }
</style>
