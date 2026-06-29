<script lang="ts">
	import { createProject, getOrgTierInfo } from '$lib/remote/projects.remote';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';

	let slug = $state('');
	let displayName = $state('');
	let storageGi = $state(10);
	let submitting = $state(false);
	let submitError = $state<string | null>(null);

	async function submit() {
		submitting = true;
		submitError = null;
		try {
			const result = await createProject({ slug, displayName, storageGi });
			await goto(`/${page.params.namespace}/projects/${result.slug}`);
		} catch (e: any) {
			submitError = e?.body?.message ?? e?.message ?? 'Failed to create project';
		} finally {
			submitting = false;
		}
	}
</script>

<div class="page-header">
	<a href="/{page.params.namespace}/projects" class="back">← Projects</a>
	<h2>New Project</h2>
</div>

{#await getOrgTierInfo()}
	<p class="muted">Loading…</p>
{:then { limits, tier }}
	<div class="new-project-form card">
		<div class="field">
			<label for="displayName">Display name</label>
			<input id="displayName" type="text" bind:value={displayName} required placeholder="My Awesome Project" />
		</div>

		<div class="field">
			<label for="slug">Slug</label>
			<input id="slug" type="text" bind:value={slug} required pattern="[a-z0-9-]+" placeholder="my-awesome-project" />
			<span class="hint">Lowercase letters, numbers, and dashes only</span>
		</div>

		<div class="field">
			<label for="storageGi">Workspace storage (GiB)</label>
			<input id="storageGi" type="number" bind:value={storageGi} min="1" max={limits.maxPvcGi} />
			<span class="hint">Max {limits.maxPvcGi} GiB on {tier} tier</span>
		</div>

		{#if submitError}
			<div class="error">{submitError}</div>
		{/if}

		<div class="actions">
			<a href="/{page.params.namespace}/projects" class="btn">Cancel</a>
			<button type="button" class="btn btn-primary" onclick={submit} disabled={submitting}>
				{submitting ? 'Creating…' : 'Create project'}
			</button>
		</div>
	</div>
{:catch e}
	<p class="error">{e?.message ?? 'Failed to load tier info'}</p>
{/await}

<style>
	.page-header { margin-bottom: 1.5rem; }
	.back { color: var(--color-text-muted); font-size: 13px; display: block; margin-bottom: 0.5rem; }
	.new-project-form { max-width: 560px; }
	.field { margin-bottom: 1.25rem; }
	label { display: block; font-weight: 500; margin-bottom: 0.375rem; }
	.hint { font-size: 12px; color: var(--color-text-muted); }
	.actions { display: flex; gap: 0.75rem; justify-content: flex-end; margin-top: 1.5rem; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.error { color: var(--color-danger); padding: 0.75rem; background: #2a1a1a; border-radius: var(--radius); margin-bottom: 1rem; }
</style>
