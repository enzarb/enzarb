<script lang="ts">
	import type { LayoutData } from './$types';
	let { data, children }: { data: LayoutData; children: any } = $props();
	const { session } = data;
</script>

<div class="shell">
	<nav class="sidebar">
		<div class="sidebar-top">
			<a href="/dashboard" class="logo">enzarb</a>
			<ul class="nav-list">
				<li><a href="/dashboard">Dashboard</a></li>
				{#each session.orgs as org}
					<li class="org-group">
						<span class="org-label">{org.slug}</span>
						<ul>
							<li><a href="/orgs/{org.id}/projects">Projects</a></li>
							<li><a href="/orgs/{org.id}/billing">Billing</a></li>
							<li><a href="/orgs/{org.id}/settings">Settings</a></li>
						</ul>
					</li>
				{/each}
				{#if session.isAdmin}
					<li><a href="/admin">Admin</a></li>
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
		margin-bottom: 1.5rem;
		letter-spacing: -0.04em;
	}
	.nav-list { list-style: none; margin: 0; padding: 0; }
	.nav-list li { margin-bottom: 0.25rem; }
	.nav-list a {
		display: block;
		padding: 0.375rem 0.5rem;
		border-radius: 4px;
		color: var(--color-text-muted);
		font-size: 13px;
	}
	.nav-list a:hover { background: var(--color-surface-2); color: var(--color-text); text-decoration: none; }
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
	.content { flex: 1; overflow-y: auto; padding: 2rem; }
	.user-email { display: block; font-size: 12px; color: var(--color-text-muted); margin-bottom: 0.5rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
