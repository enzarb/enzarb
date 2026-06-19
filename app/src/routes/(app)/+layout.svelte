<script lang="ts">
	import type { LayoutData } from './$types';
	import { afterNavigate } from '$app/navigation';

	let { data, children }: { data: LayoutData; children: any } = $props();
	const { session } = data;

	let open = $state(false);

	// Close the mobile drawer whenever navigation completes.
	afterNavigate(() => {
		open = false;
	});
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
		<a href="/dashboard" class="logo">enzarb</a>
	</header>

	{#if open}
		<button class="backdrop" aria-label="Close navigation" onclick={() => (open = false)}></button>
	{/if}

	<nav class="sidebar" class:open>
		<div class="sidebar-top">
			<a href="/dashboard" class="logo logo-desktop">enzarb</a>
			<ul class="nav-list">
				<li>
					<a href="/dashboard">
						<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/></svg>
						<span>Dashboard</span>
					</a>
				</li>
				{#each session.orgs as org}
					<li class="org-group">
						<span class="org-label">{org.slug}</span>
						<ul>
							<li>
								<a href="/{org.slug}/projects">
									<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
									<span>Projects</span>
								</a>
							</li>
							<li>
								<a href="/{org.slug}/billing">
									<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="1" y="4" width="22" height="16" rx="2"/><line x1="1" y1="10" x2="23" y2="10"/></svg>
									<span>Billing</span>
								</a>
							</li>
							<li>
								<a href="/{org.slug}/settings">
									<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
									<span>Settings</span>
								</a>
							</li>
						</ul>
					</li>
				{/each}
				{#if session.isAdmin}
					<li>
						<a href="/admin">
							<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/></svg>
							<span>Admin</span>
						</a>
					</li>
				{/if}
			</ul>
		</div>
		<div class="sidebar-bottom">
			<span class="user-email">{session.email}</span>
			<form method="POST" action="/auth/logout">
				<button type="submit" class="btn">Sign out</button>
			</form>
		</div>
	</nav>
	<main class="content">
		{@render children()}
	</main>
</div>

<style>
	.shell {
		display: flex;
		min-height: 100vh;
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
	.sidebar-top { flex: 1; padding: 0 1rem; }
	.sidebar-bottom { padding: 1rem; border-top: 1px solid var(--color-border); }
	.logo {
		display: block;
		font-size: 1.25rem;
		font-weight: 700;
		color: var(--color-text);
		letter-spacing: -0.04em;
	}
	.logo:hover { text-decoration: none; }
	.logo-desktop { margin-bottom: 1.5rem; }
	.nav-list { list-style: none; margin: 0; padding: 0; }
	.nav-list li { margin-bottom: 0.25rem; }
	.nav-list a {
		display: flex;
		align-items: center;
		gap: 0.625rem;
		padding: 0.375rem 0.5rem;
		border-radius: 4px;
		color: var(--color-text-muted);
		font-size: 13px;
	}
	.nav-list a:hover { background: var(--color-surface-2); color: var(--color-text); text-decoration: none; }
	.icon { width: 16px; height: 16px; flex-shrink: 0; }
	.org-group { margin-top: 1rem; }
	.org-label {
		font-size: 11px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--color-text-muted);
		padding: 0 0.5rem;
	}
	.org-group ul { list-style: none; margin: 0.25rem 0 0; padding: 0 0 0 0.75rem; }
	.content { flex: 1; overflow-y: auto; padding: 2rem; min-width: 0; }
	.user-email { display: block; font-size: 12px; color: var(--color-text-muted); margin-bottom: 0.5rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

	@media (max-width: 768px) {
		.shell { flex-direction: column; }
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
