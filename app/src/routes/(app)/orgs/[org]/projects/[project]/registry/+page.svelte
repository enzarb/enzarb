<script lang="ts">
	import type { PageData } from './$types';
	let { data }: { data: PageData } = $props();
	let selectedRepo: string | null = $state(null);
	let tags: string[] = $state([]);
	let loadingTags = $state(false);

	async function loadTags(repo: string) {
		selectedRepo = repo;
		loadingTags = true;
		const res = await fetch(`/api/registry/tags?repo=${encodeURIComponent(repo)}&org=${data.org?.id}`);
		if (res.ok) tags = (await res.json()).tags ?? [];
		loadingTags = false;
	}

	const registryBase = 'registry.enzarb.dev';
</script>

<div class="registry-page">
	<div class="registry-layout">
		<div class="repo-list card">
			<h3>Repositories</h3>
			{#each data.repos as repo}
				<button class="repo-item {selectedRepo === repo.name ? 'active' : ''}" onclick={() => loadTags(repo.name)}>
					{repo.name}
				</button>
			{:else}
				<p class="muted">No images yet.</p>
				<p class="muted hint">Push an image with:</p>
				<pre class="code">docker push {registryBase}/&lt;org&gt;/&lt;name&gt;:tag</pre>
			{/each}
		</div>

		{#if selectedRepo}
			<div class="tag-list card">
				<h3>{selectedRepo}</h3>
				{#if loadingTags}
					<p class="muted">Loading tags…</p>
				{:else}
					<table>
						<thead><tr><th>Tag</th><th>Pull command</th></tr></thead>
						<tbody>
							{#each tags as tag}
								<tr>
									<td><span class="badge">{tag}</span></td>
									<td><code class="mono small">docker pull {registryBase}/{selectedRepo}:{tag}</code></td>
								</tr>
							{:else}
								<tr><td colspan="2" class="muted">No tags</td></tr>
							{/each}
						</tbody>
					</table>
				{/if}
			</div>
		{/if}
	</div>
</div>

<style>
	.registry-layout { display: grid; grid-template-columns: 260px 1fr; gap: 1rem; }
	.repo-list h3, .tag-list h3 { margin-bottom: 0.75rem; font-size: 14px; }
	.repo-item { display: block; width: 100%; text-align: left; padding: 0.375rem 0.5rem; border-radius: 4px; border: none; background: none; color: var(--color-text-muted); font-size: 13px; cursor: pointer; font-family: var(--font-mono); }
	.repo-item:hover { background: var(--color-surface-2); color: var(--color-text); }
	.repo-item.active { background: var(--color-accent-dim); color: var(--color-text); }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.hint { margin-top: 0.5rem; }
	.code { font-family: var(--font-mono); font-size: 12px; background: var(--color-surface-2); padding: 0.5rem; border-radius: 4px; }
	.mono { font-family: var(--font-mono); }
	.small { font-size: 12px; }
</style>
