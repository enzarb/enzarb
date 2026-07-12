<script lang="ts">
	import type { LayoutData } from './$types';
	import { afterNavigate } from '$app/navigation';
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import ConfirmDialog from '$lib/components/ConfirmDialog.svelte';

	let { data, children }: { data: LayoutData; children: any } = $props();
	const session = $derived(data.session);
	const orgProjects = $derived(data.orgProjects);

	let open = $state(false);
	let theme = $state('dark');

	afterNavigate(() => {
		open = false;
	});

	function initTheme() {
		const saved = localStorage.getItem('theme') ?? 'dark';
		theme = saved;
		document.documentElement.setAttribute('data-theme', saved);
	}

	function toggleTheme() {
		theme = theme === 'dark' ? 'light' : 'dark';
		localStorage.setItem('theme', theme);
		document.documentElement.setAttribute('data-theme', theme);
	}

	function isProjectActive(orgSlug: string, projectSlug: string) {
		const base = `/${orgSlug}/projects/${projectSlug}`;
		return page.url.pathname === base || page.url.pathname.startsWith(base + '/');
	}

	function isPathActive(path: string) {
		return page.url.pathname === path || page.url.pathname.startsWith(path + '/');
	}

	onMount(initTheme);
</script>


<div class="shell">
	<header class="topbar">
		<button
			class="menu-btn"
			aria-label="Toggle navigation"
			aria-expanded={open}
			onclick={() => (open = !open)}
		>
			<svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
				{#if open}
					<path d="M6 6l12 12M18 6L6 18" />
				{:else}
					<path d="M4 6h16M4 12h16M4 18h16" />
				{/if}
			</svg>
		</button>
		<a href="/" class="logo">enzarb</a>
	</header>

	{#if open}
		<button class="backdrop" aria-label="Close navigation" onclick={() => (open = false)}></button>
	{/if}

	<nav class="sidebar" class:open>
		<div class="sidebar-top">
			<a href="/" class="logo logo-desktop">enzarb</a>
			{#each session.orgs as org}
				{@const projects = orgProjects[org.slug] ?? []}
				<div class="org-section">
					<div class="org-header">
						<span class="org-label">{org.slug}</span>
						<div class="org-actions">
							<a href="/{org.slug}/projects/new" class="icon-btn" title="New project" aria-label="New project">
								<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>
							</a>
						</div>
					</div>
					<ul class="project-list">
						{#each projects as project}
							<li>
								<a
									href="/{org.slug}/projects/{project.slug}"
									class="project-link"
									class:active={isProjectActive(org.slug, project.slug)}
								>
									<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
									{project.displayName}
								</a>
							</li>
						{:else}
							<li class="no-projects">
								<a href="/{org.slug}/projects/new" class="project-link muted">+ New project</a>
							</li>
						{/each}
					</ul>
				</div>
			{/each}
			<div class="bottom-links">
				<a href="/tiling" class="bottom-link" class:active={isPathActive('/tiling')}>
					<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><line x1="12" y1="3" x2="12" y2="21"/><line x1="12" y1="12" x2="21" y2="12"/></svg>
					Tiling
				</a>
				{#each session.orgs as org}
					<a href="/{org.slug}/settings" class="bottom-link" class:active={isPathActive(`/${org.slug}/settings`)}>
						<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
						Settings
					</a>
					<a href="/{org.slug}/billing" class="bottom-link" class:active={isPathActive(`/${org.slug}/billing`)}>
						<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="1" y="4" width="22" height="16" rx="2"/><line x1="1" y1="10" x2="23" y2="10"/></svg>
						Billing
					</a>
				{/each}
				{#if session.isAdmin}
					<a href="/admin" class="bottom-link" class:active={isPathActive('/admin')}>
						<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/></svg>
						Admin
					</a>
				{/if}
			</div>
		</div>
		<div class="sidebar-bottom">
			<div class="sidebar-bottom-row">
				<button class="icon-btn" title={theme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'} onclick={toggleTheme}>
					{#if theme === 'dark'}
						<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="5"/><path d="M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42"/></svg>
					{:else}
						<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>
					{/if}
				</button>
				<div class="user-pill" title={session.email}>
					<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/></svg>
					<span class="user-email">{session.email}</span>
				</div>
				<form method="POST" action="/auth/logout">
					<button type="submit" class="icon-btn" title="Sign out">
						<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/></svg>
					</button>
				</form>
			</div>
		</div>
	</nav>
	<main class="content">
		{@render children()}
	</main>
</div>

<ConfirmDialog />

<style>
	.shell {
		display: flex;
		height: 100vh;
		overflow: hidden;
	}
	.topbar {
		display: none;
		align-items: center;
		gap: 0.75rem;
		height: 52px;
		padding: 0 0.75rem;
		background: var(--color-surface);
		border-bottom: 1px solid var(--color-border);
		position: sticky;
		top: 0;
		z-index: 30;
	}
	.menu-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 36px;
		height: 36px;
		border: 1px solid var(--color-border);
		border-radius: var(--radius);
		background: var(--color-surface-2);
		color: var(--color-text);
	}
	.backdrop {
		display: none;
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.5);
		border: none;
		z-index: 39;
	}
	.sidebar {
		width: 220px;
		flex-shrink: 0;
		background: var(--color-surface);
		border-right: 1px solid var(--color-border);
		display: flex;
		flex-direction: column;
		padding: 1rem 0;
	}
	.sidebar-top { flex: 1; padding: 0 0.75rem; overflow-y: auto; }
	.sidebar-bottom { padding: 0.75rem; border-top: 1px solid var(--color-border); }
	.sidebar-bottom-row { display: flex; align-items: center; gap: 0.25rem; }
	.user-pill { display: flex; align-items: center; gap: 0.35rem; flex: 1; min-width: 0; color: var(--color-text-muted); font-size: 12px; overflow: hidden; }
	.sidebar-bottom-row form { display: contents; }
	.logo {
		display: block;
		font-size: 1.25rem;
		font-weight: 700;
		color: var(--color-text);
		letter-spacing: -0.04em;
	}
	.logo:hover { text-decoration: none; }
	.logo-desktop { margin-bottom: 1.25rem; }

	/* Org section */
	.org-section { margin-bottom: 1.25rem; }
	.org-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 0 0.25rem;
		margin-bottom: 0.25rem;
	}
	.org-label {
		font-size: 11px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--color-text-muted);
	}
	.org-actions { display: flex; align-items: center; gap: 0.125rem; }
	.icon-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 22px;
		height: 22px;
		border: none;
		background: none;
		color: var(--color-text-muted);
		border-radius: 4px;
		cursor: pointer;
		padding: 0;
		text-decoration: none;
	}
	.icon-btn:hover { background: var(--color-surface-2); color: var(--color-text); text-decoration: none; }

	/* Project list */
	.project-list { list-style: none; margin: 0; padding: 0; }
	.project-list li { margin-bottom: 1px; }
	.project-link {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.3rem 0.5rem;
		border-radius: 4px;
		color: var(--color-text-muted);
		font-size: 13px;
		text-decoration: none;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}
	.project-link:hover { background: var(--color-surface-2); color: var(--color-text); text-decoration: none; }
	.project-link.active { background: var(--color-surface-2); color: var(--color-text); }
	.project-link.muted { font-style: italic; }
	.icon { width: 14px; height: 14px; flex-shrink: 0; }

	.bottom-links { margin-top: 1rem; padding-top: 1rem; border-top: 1px solid var(--color-border); display: flex; flex-direction: column; gap: 1px; }
	.bottom-link {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		padding: 0.3rem 0.5rem;
		border-radius: 4px;
		color: var(--color-text-muted);
		font-size: 13px;
		text-decoration: none;
	}
	.bottom-link:hover { background: var(--color-surface-2); color: var(--color-text); text-decoration: none; }
	.bottom-link.active { background: var(--color-surface-2); color: var(--color-text); }

	.content { flex: 1; overflow-y: auto; padding: 2rem; min-width: 0; }
	.user-email { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

	@media (max-width: 768px) {
		.shell { flex-direction: column; height: 100vh; height: 100svh; overflow: hidden; }
		.topbar { display: flex; }
		.logo-desktop { display: none; }
		.backdrop { display: block; }
		.sidebar {
			position: fixed;
			top: 0;
			left: 0;
			bottom: 0;
			z-index: 40;
			transform: translateX(-100%);
			transition: transform 0.2s ease;
			box-shadow: var(--shadow);
		}
		.sidebar.open { transform: translateX(0); }
		.content { padding: 1rem; }
	}
</style>
