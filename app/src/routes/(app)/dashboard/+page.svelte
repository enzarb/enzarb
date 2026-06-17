<script lang="ts">
	import type { PageData } from './$types';
	let { data }: { data: PageData } = $props();
</script>

<h2>Organizations</h2>
<div class="orgs">
	{#each data.orgs as org}
		<a href="/orgs/{org.id}/projects" class="card org-card">
			<div class="org-name">{org.slug}</div>
			<div class="org-role badge">{org.role}</div>
		</a>
	{:else}
		<p class="empty">You are not a member of any organization yet. Contact an admin.</p>
	{/each}
</div>

<style>
	.orgs { display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 1rem; margin-top: 1rem; }
	.org-card { display: flex; justify-content: space-between; align-items: center; text-decoration: none; color: var(--color-text); }
	.org-card:hover { border-color: var(--color-accent); text-decoration: none; }
	.org-name { font-weight: 600; }
	.empty { color: var(--color-text-muted); }
</style>
