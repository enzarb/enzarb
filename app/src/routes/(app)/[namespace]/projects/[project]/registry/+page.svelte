<script lang="ts">
	import { getRepositories, getRepoTagSizes } from '$lib/remote/registry.remote';
	import { page } from '$app/state';

	type TagRow = { tag: string; totalSize: number; uniqueSize: number; createdAt: string | null };

	let selectedRepo: string | null = $state(null);
	let tagSizes: TagRow[] = $state([]);
	let summary: { totalUniqueBytes: number; naiveSumBytes: number } | null = $state(null);
	let loadingTags = $state(false);
	let copiedTag: string | null = $state(null);
	let sortKey: 'tag' | 'createdAt' = $state('tag');
	let sortDesc = $state(false);

	const sortedTagSizes = $derived(
		[...tagSizes].sort((a, b) => {
			let cmp: number;
			if (sortKey === 'createdAt') {
				cmp = (a.createdAt ? Date.parse(a.createdAt) : 0) - (b.createdAt ? Date.parse(b.createdAt) : 0);
			} else {
				cmp = a.tag.localeCompare(b.tag);
			}
			return sortDesc ? -cmp : cmp;
		})
	);

	function setSort(key: 'tag' | 'createdAt') {
		if (sortKey === key) {
			sortDesc = !sortDesc;
		} else {
			sortKey = key;
			sortDesc = key === 'createdAt';
		}
	}

	async function loadTags(repo: string) {
		selectedRepo = repo;
		loadingTags = true;
		try {
			const result = await getRepoTagSizes(repo);
			tagSizes = result.tags;
			summary = { totalUniqueBytes: result.totalUniqueBytes, naiveSumBytes: result.naiveSumBytes };
		} finally {
			loadingTags = false;
		}
	}

	async function copyImagePath(path: string) {
		await navigator.clipboard.writeText(path);
		copiedTag = path;
		setTimeout(() => { copiedTag = null; }, 1500);
	}

	function formatBytes(bytes: number): string {
		if (!bytes) return '—';
		const units = ['B', 'KB', 'MB', 'GB', 'TB'];
		const i = Math.min(Math.floor(Math.log2(bytes) / 10), units.length - 1);
		return (bytes / Math.pow(1024, i)).toLocaleString('en-US', { maximumFractionDigits: 2 }) + ' ' + units[i];
	}

	function formatDate(iso: string | null): string {
		if (!iso) return '—';
		const d = new Date(iso);
		if (isNaN(d.getTime())) return '—';
		return d.toLocaleDateString('en-US', { year: 'numeric', month: 'short', day: 'numeric' });
	}

	const registryBase = 'registry.enzarb.dev';
	const registryPrefix = $derived(`${registryBase}/${page.params.namespace}/${page.params.project}`);
</script>

{#await getRepositories() then repos}
	<div class="registry-page">
		<div class="push-guide card">
			<h3>Push images</h3>
			<p class="muted">Set <code>REGISTRY</code> to your project's registry prefix, then build and push normally.</p>
			<pre class="code">export REGISTRY={registryPrefix}

docker build -t $REGISTRY/&lt;image&gt;:&lt;tag&gt; .
docker push $REGISTRY/&lt;image&gt;:&lt;tag&gt;</pre>
		</div>

		<div class="registry-layout">
			<div class="repo-list card">
				<h3>Repositories</h3>
				{#each repos as repo}
					<button class="repo-item {selectedRepo === repo.name ? 'active' : ''}" onclick={() => loadTags(repo.name)}>
						{repo.name}
					</button>
				{:else}
					<p class="muted">No images yet.</p>
				{/each}
			</div>

			{#if selectedRepo}
				<div class="tag-list card">
					<h3>{selectedRepo}</h3>
					{#if loadingTags}
						<p class="muted">Loading tags…</p>
					{:else}
						{#if summary && tagSizes.length > 0}
							{@const savingsPct = summary.naiveSumBytes > 0
								? Math.round((1 - summary.totalUniqueBytes / summary.naiveSumBytes) * 100)
								: 0}
							<div class="storage-summary">
								<div class="summary-stat">
									<span class="summary-label">Unique storage used</span>
									<span class="summary-value">{formatBytes(summary.totalUniqueBytes)}</span>
								</div>
								<div class="summary-stat">
									<span class="summary-label">If tags were independent</span>
									<span class="summary-value muted">{formatBytes(summary.naiveSumBytes)}</span>
								</div>
								{#if savingsPct > 0}
									<div class="summary-stat">
										<span class="summary-label">Saved via shared layers</span>
										<span class="summary-value savings">{savingsPct}%</span>
									</div>
								{/if}
							</div>
						{/if}
						<table>
							<thead>
								<tr>
									<th class="sortable" onclick={() => setSort('tag')}>
										Tag {sortKey === 'tag' ? (sortDesc ? '↓' : '↑') : ''}
									</th>
									<th>Size</th>
									<th>Unique</th>
									<th class="sortable" onclick={() => setSort('createdAt')}>
										Created {sortKey === 'createdAt' ? (sortDesc ? '↓' : '↑') : ''}
									</th>
									<th></th>
								</tr>
							</thead>
							<tbody>
								{#each sortedTagSizes as t}
									{@const imagePath = `${registryBase}/${selectedRepo}:${t.tag}`}
									<tr>
										<td><span class="badge">{t.tag}</span></td>
										<td class="mono small">{formatBytes(t.totalSize)}</td>
										<td class="mono small muted" title="Storage not shared with any other tag in this repository">
											{t.uniqueSize > 0 ? formatBytes(t.uniqueSize) : '—'}
										</td>
										<td class="mono small muted">{formatDate(t.createdAt)}</td>
										<td>
											<button class="copy-btn" onclick={() => copyImagePath(imagePath)} title="Copy image path">
												{copiedTag === imagePath ? '✓ Copied' : '⎘ Copy'}
											</button>
										</td>
									</tr>
								{:else}
									<tr><td colspan="5" class="muted">No tags</td></tr>
								{/each}
							</tbody>
						</table>
					{/if}
				</div>
			{/if}
		</div>
	</div>
{:catch err}
	<p class="muted">Could not load registry: {err?.message ?? err?.status ?? String(err) ?? 'unknown error'}</p>
{/await}

<style>
	.push-guide { margin-bottom: 1rem; }
	.push-guide h3 { margin-bottom: 0.5rem; font-size: 14px; }
	.push-guide p { margin-bottom: 0.5rem; }
	.push-guide code { font-family: var(--font-mono); font-size: 12px; }
	.registry-layout { display: grid; grid-template-columns: 260px 1fr; gap: 1rem; }
	.repo-list h3, .tag-list h3 { margin-bottom: 0.75rem; font-size: 14px; }
	.repo-item { display: block; width: 100%; text-align: left; padding: 0.375rem 0.5rem; border-radius: 4px; border: none; background: none; color: var(--color-text-muted); font-size: 13px; cursor: pointer; font-family: var(--font-mono); }
	.repo-item:hover { background: var(--color-surface-2); color: var(--color-text); }
	.repo-item.active { background: var(--color-accent-dim); color: var(--color-text); }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.code { font-family: var(--font-mono); font-size: 12px; background: var(--color-surface-2); padding: 0.5rem; border-radius: 4px; }
	.mono { font-family: var(--font-mono); }
	.small { font-size: 12px; }
	.storage-summary { display: flex; gap: 1.5rem; margin-bottom: 1rem; padding: 0.75rem 1rem; background: var(--color-surface-2); border-radius: 6px; }
	.summary-stat { display: flex; flex-direction: column; gap: 0.15rem; }
	.summary-label { font-size: 10px; text-transform: uppercase; letter-spacing: 0.05em; color: var(--color-text-muted); }
	.summary-value { font-size: 15px; font-weight: 600; font-family: var(--font-mono); }
	.summary-value.savings { color: #3fb950; }
	.copy-btn { background: none; border: 1px solid var(--color-border); border-radius: 4px; cursor: pointer; padding: 0.2rem 0.5rem; font-size: 11px; color: var(--color-text-muted); }
	.copy-btn:hover { color: var(--color-text); border-color: var(--color-text-muted); }
	th.sortable { cursor: pointer; user-select: none; }
	th.sortable:hover { color: var(--color-text); }
</style>
