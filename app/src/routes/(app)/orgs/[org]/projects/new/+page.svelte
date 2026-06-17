<script lang="ts">
	import type { PageData } from './$types';
	import { enhance } from '$app/forms';
	let { data }: { data: PageData } = $props();

	const availableTools = [
		{ name: 'claude', label: 'Claude Code' },
		{ name: 'node', label: 'Node.js' },
		{ name: 'python', label: 'Python' },
		{ name: 'go', label: 'Go' },
		{ name: 'rust', label: 'Rust' },
		{ name: 'java', label: 'Java' },
		{ name: 'kubectl', label: 'kubectl' },
		{ name: 'terraform', label: 'Terraform' },
		{ name: 'helm', label: 'Helm' }
	];

	let selectedTools: string[] = $state([]);
	let form = $state({ slug: '', displayName: '', storageGi: 10, error: '' });

	function toggleTool(name: string) {
		if (selectedTools.includes(name)) {
			selectedTools = selectedTools.filter((t) => t !== name);
		} else {
			selectedTools = [...selectedTools, name];
		}
	}
</script>

<div class="page-header">
	<a href="/orgs/{data.org.id}/projects" class="back">← Projects</a>
	<h2>New Project</h2>
</div>

<form method="POST" use:enhance class="new-project-form card">
	<div class="field">
		<label for="displayName">Display name</label>
		<input id="displayName" name="displayName" type="text" required placeholder="My Awesome Project" />
	</div>

	<div class="field">
		<label for="slug">Slug</label>
		<input id="slug" name="slug" type="text" required pattern="[a-z0-9-]+" placeholder="my-awesome-project" />
		<span class="hint">Lowercase letters, numbers, and dashes only</span>
	</div>

	<div class="field">
		<label>Tools <span class="hint">(installed via mise on first boot)</span></label>
		<div class="tool-grid">
			{#each availableTools as tool}
				<button
					type="button"
					class="tool-btn {selectedTools.includes(tool.name) ? 'selected' : ''}"
					onclick={() => toggleTool(tool.name)}
				>
					{tool.label}
				</button>
			{/each}
		</div>
		{#each selectedTools as tool}
			<input type="hidden" name="tools" value={tool} />
		{/each}
	</div>

	<div class="field">
		<label for="storageGi">Workspace storage (GiB)</label>
		<input id="storageGi" name="storageGi" type="number" min="1" max="{data.limits.maxPvcGi}" value={form.storageGi} />
		<span class="hint">Max {data.limits.maxPvcGi} GiB on {data.org.tier} tier</span>
	</div>

	{#if data.form?.error}
		<div class="error">{data.form.error}</div>
	{/if}

	<div class="actions">
		<a href="/orgs/{data.org.id}/projects" class="btn">Cancel</a>
		<button type="submit" class="btn btn-primary">Create project</button>
	</div>
</form>

<style>
	.page-header { margin-bottom: 1.5rem; }
	.back { color: var(--color-text-muted); font-size: 13px; display: block; margin-bottom: 0.5rem; }
	.new-project-form { max-width: 560px; }
	.field { margin-bottom: 1.25rem; }
	label { display: block; font-weight: 500; margin-bottom: 0.375rem; }
	.hint { font-size: 12px; color: var(--color-text-muted); }
	.tool-grid { display: flex; flex-wrap: wrap; gap: 0.5rem; margin-top: 0.5rem; }
	.tool-btn {
		padding: 0.375rem 0.75rem;
		border-radius: var(--radius);
		border: 1px solid var(--color-border);
		background: var(--color-surface-2);
		color: var(--color-text-muted);
		font-size: 13px;
		cursor: pointer;
	}
	.tool-btn.selected {
		border-color: var(--color-accent);
		background: var(--color-accent-dim);
		color: var(--color-text);
	}
	.actions { display: flex; gap: 0.75rem; justify-content: flex-end; margin-top: 1.5rem; }
	.error { color: var(--color-danger); padding: 0.75rem; background: #2a1a1a; border-radius: var(--radius); margin-bottom: 1rem; }
</style>
