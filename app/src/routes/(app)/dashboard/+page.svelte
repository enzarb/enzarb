<script lang="ts">
	import { createOrg } from '$lib/remote/orgs.remote';
	import type { LayoutData } from '../$types';

	let { data }: { data: LayoutData } = $props();
	let showNewOrg = $state(false);
</script>

<div class="dashboard-header">
	<h2>Organizations</h2>
	<button class="btn btn-primary" onclick={() => (showNewOrg = !showNewOrg)}>
		{showNewOrg ? 'Cancel' : 'New organization'}
	</button>
</div>

{#if showNewOrg}
	<div class="card new-org-form">
		<h3>Create organization</h3>
		<form {...createOrg}>
			<div class="field">
				<label for="org-name">Display name</label>
				<input id="org-name" {...createOrg.fields.displayName.as('text')} placeholder="Acme Corp" required />
				{#each createOrg.fields.displayName.issues() as issue}<p class="field-error">{issue.message}</p>{/each}
			</div>
			<div class="field">
				<label for="org-slug">Slug</label>
				<input id="org-slug" {...createOrg.fields.slug.as('text')} placeholder="acme-corp" pattern="[a-z0-9][a-z0-9\-]*[a-z0-9]" minlength="2" maxlength="63" required />
				<p class="hint">Lowercase letters, numbers, hyphens. Used in URLs.</p>
				{#each createOrg.fields.slug.issues() as issue}<p class="field-error">{issue.message}</p>{/each}
			</div>
			<button type="submit" class="btn btn-primary">Create</button>
		</form>
	</div>
{/if}

<div class="orgs">
	{#each data.session.orgs as org}
		<a href="/orgs/{org.id}/projects" class="card org-card">
			<div class="org-name">{org.slug}</div>
			<div class="org-role badge">{org.role}</div>
		</a>
	{:else}
		<div class="empty-state">
			<p>You're not a member of any organization yet.</p>
			<button class="btn btn-primary" onclick={() => (showNewOrg = true)}>Create your first organization</button>
		</div>
	{/each}
</div>

<style>
	.dashboard-header { display: flex; align-items: center; justify-content: space-between; margin-bottom: 1rem; }
	.orgs { display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 1rem; margin-top: 1rem; }
	.org-card { display: flex; justify-content: space-between; align-items: center; text-decoration: none; color: var(--color-text); }
	.org-card:hover { border-color: var(--color-accent); text-decoration: none; }
	.org-name { font-weight: 600; }
	.empty-state { grid-column: 1 / -1; text-align: center; padding: 3rem 1rem; color: var(--color-text-muted); }
	.empty-state p { margin: 0 0 1rem; }
	.new-org-form { margin-bottom: 1.5rem; max-width: 480px; }
	.new-org-form h3 { margin: 0 0 1.25rem; }
	.field { margin-bottom: 1rem; }
	.field label { display: block; font-weight: 500; font-size: 13px; margin-bottom: 0.375rem; }
	.hint { font-size: 12px; color: var(--color-text-muted); margin: 0.25rem 0 0; }
	.field-error { font-size: 13px; color: var(--color-danger); margin: 0.25rem 0 0; }
</style>
