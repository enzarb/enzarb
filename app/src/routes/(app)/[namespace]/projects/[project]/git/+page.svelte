<script lang="ts">
	import { getGitContents, getGitRefs } from '$lib/remote/files.remote';
	import { page } from '$app/stores';

	let ref = $state('main');
	let path = $state('');

	function parentPath(p: string) {
		return p.split('/').filter(Boolean).slice(0, -1).join('/');
	}

	const breadcrumbs = $derived(path ? path.split('/').filter(Boolean) : []);

	function fmtSize(b?: number) {
		if (!b) return '';
		if (b < 1024) return `${b} B`;
		if (b < 1048576) return `${(b / 1024).toFixed(1)} KB`;
		return `${(b / 1048576).toFixed(1)} MB`;
	}
</script>

{#await getGitRefs() then { branches, tags }}
	<div class="git-page">
		<div class="toolbar">
			<nav class="breadcrumb">
				<button class="crumb" onclick={() => { path = ''; }}>
					{$page.params.project}
				</button>
				{#each breadcrumbs as part, i}
					<span class="sep">/</span>
					<button class="crumb" onclick={() => { path = breadcrumbs.slice(0, i + 1).join('/'); }}>
						{part}
					</button>
				{/each}
			</nav>
			<select class="ref-select" bind:value={ref} onchange={() => { path = ''; }}>
				{#if branches.length}
					<optgroup label="Branches">
						{#each branches as b}<option value={b}>{b}</option>{/each}
					</optgroup>
				{/if}
				{#if tags.length}
					<optgroup label="Tags">
						{#each tags as t}<option value={t}>{t}</option>{/each}
					</optgroup>
				{/if}
			</select>
		</div>

		{#await getGitContents({ path, ref }) then entries}
			{@const list = Array.isArray(entries) ? entries : entries ? [entries] : []}
			{@const dirs = list.filter((e: any) => e.type === 'dir').sort((a: any, b: any) => a.name.localeCompare(b.name))}
			{@const files = list.filter((e: any) => e.type !== 'dir').sort((a: any, b: any) => a.name.localeCompare(b.name))}
			<table class="file-table">
				<thead>
					<tr><th>Name</th><th>Size</th></tr>
				</thead>
				<tbody>
					{#if path}
						<tr>
							<td colspan="2">
								<button class="entry-btn" onclick={() => { path = parentPath(path); }}>
									<span class="icon">⬆</span> ..
								</button>
							</td>
						</tr>
					{/if}
					{#each [...dirs, ...files] as entry (entry.sha)}
						<tr>
							<td>
								{#if entry.type === 'dir'}
									<button class="entry-btn dir" onclick={() => { path = entry.path; }}>
										<span class="icon">📁</span>{entry.name}
									</button>
								{:else}
									<span class="entry-name"><span class="icon">📄</span>{entry.name}</span>
								{/if}
							</td>
							<td class="muted">{fmtSize(entry.size)}</td>
						</tr>
					{:else}
						<tr><td colspan="2" class="muted empty">Repository is empty</td></tr>
					{/each}
				</tbody>
			</table>
		{:catch}
			<p class="muted">Could not load repository contents.</p>
		{/await}
	</div>
{:catch}
	<p class="muted">Could not load repository — it may not be initialized yet.</p>
{/await}

<style>
	.git-page { display: flex; flex-direction: column; gap: 0.75rem; }
	.toolbar { display: flex; justify-content: space-between; align-items: center; gap: 1rem; padding-bottom: 0.5rem; border-bottom: 1px solid var(--color-border); }
	.breadcrumb { display: flex; align-items: center; gap: 0.2rem; font-family: var(--font-mono); font-size: 13px; flex-wrap: wrap; }
	.crumb { background: none; border: none; color: var(--color-accent); cursor: pointer; padding: 0 0.1rem; font-family: var(--font-mono); font-size: 13px; }
	.crumb:hover { text-decoration: underline; }
	.sep { color: var(--color-text-muted); }
	.ref-select { padding: 0.3rem 0.5rem; border: 1px solid var(--color-border); border-radius: var(--radius); background: var(--color-surface-2); color: var(--color-text); font-size: 13px; font-family: var(--font-mono); }
	.file-table { width: 100%; border-collapse: collapse; }
	.file-table th { text-align: left; font-size: 11px; text-transform: uppercase; color: var(--color-text-muted); font-weight: 500; padding: 0.25rem 0.5rem; }
	.file-table td { padding: 0.3rem 0.5rem; font-size: 13px; border-top: 1px solid var(--color-border); }
	.entry-btn { background: none; border: none; cursor: pointer; padding: 0; font-size: 13px; display: flex; align-items: center; gap: 0.4rem; color: var(--color-text); }
	.entry-btn:hover { text-decoration: underline; }
	.entry-btn.dir { color: var(--color-accent); }
	.entry-name { display: flex; align-items: center; gap: 0.4rem; font-size: 13px; }
	.icon { font-size: 14px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.empty { text-align: center; padding: 2rem 0; }
</style>
