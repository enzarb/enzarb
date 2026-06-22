<script lang="ts">
	import { getGitContents, getGitRefs, getGitCommits, getGitCommit, getGitBlame } from '$lib/remote/files.remote';
	import { page } from '$app/stores';
	import CodeViewer from '$lib/components/CodeViewer.svelte';
	import BlameViewer from '$lib/components/BlameViewer.svelte';
	import CommitList from '$lib/components/CommitList.svelte';
	import type { GiteaCommit } from '$lib/gitea';

	let ref = $state('main');
	let path = $state('');
	let selectedFile = $state<{ path: string; name: string; content: string } | null>(null);
	let fileTab = $state<'code' | 'blame' | 'history'>('code');
	let rootView = $state<'tree' | 'commits'>('tree');
	let selectedCommit = $state<any | null>(null);

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

	function openFile(entry: any) {
		const raw = entry.content ? atob(entry.content) : '';
		selectedFile = { path: entry.path, name: entry.name, content: raw };
		fileTab = 'code';
	}

	function closeFile() {
		selectedFile = null;
		selectedCommit = null;
	}

	function switchRef() {
		path = '';
		selectedFile = null;
		selectedCommit = null;
	}
</script>

{#await getGitRefs() then { branches, tags }}
	<div class="git-page">
		<div class="toolbar">
			{#if selectedFile}
				<nav class="breadcrumb">
					<button class="crumb back-btn" onclick={closeFile}>← Files</button>
					<span class="sep">/</span>
					<span class="crumb-static">{selectedFile.name}</span>
				</nav>
			{:else if rootView === 'commits' && !path}
				<nav class="breadcrumb">
					<button class="crumb back-btn" onclick={() => rootView = 'tree'}>← Tree</button>
					<span class="sep">/</span>
					<span class="crumb-static">Commits</span>
				</nav>
			{:else}
				<nav class="breadcrumb">
					<button class="crumb" onclick={() => { path = ''; selectedFile = null; }}>
						{$page.params.project}
					</button>
					{#each breadcrumbs as part, i}
						<span class="sep">/</span>
						<button class="crumb" onclick={() => { path = breadcrumbs.slice(0, i + 1).join('/'); }}>
							{part}
						</button>
					{/each}
				</nav>
			{/if}

			<div class="toolbar-right">
				{#if !selectedFile}
					<button
						class="btn-sm"
						class:active={rootView === 'commits' && !path}
						onclick={() => { rootView = 'commits'; path = ''; selectedFile = null; selectedCommit = null; }}
					>Commits</button>
				{/if}
				<select class="ref-select" bind:value={ref} onchange={switchRef}>
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
		</div>

		{#if selectedFile}
			<!-- FILE VIEWER -->
			<div class="file-viewer">
				<div class="file-tabs">
					<button class="tab" class:active={fileTab === 'code'} onclick={() => fileTab = 'code'}>Code</button>
					<button class="tab" class:active={fileTab === 'blame'} onclick={() => fileTab = 'blame'}>Blame</button>
					<button class="tab" class:active={fileTab === 'history'} onclick={() => fileTab = 'history'}>History</button>
				</div>
				{#if fileTab === 'code'}
					<CodeViewer content={selectedFile.content} filename={selectedFile.name} />
				{:else if fileTab === 'blame'}
					{#await getGitBlame({ filepath: selectedFile.path, ref })}
						<p class="muted">Loading blame…</p>
					{:then sections}
						{#if sections && sections.length}
							<BlameViewer {sections} filename={selectedFile.name} />
						{:else}
							<p class="muted">Blame data unavailable.</p>
						{/if}
					{:catch}
						<p class="muted">Could not load blame.</p>
					{/await}
				{:else}
					{#await getGitCommits({ ref, path: selectedFile.path })}
						<p class="muted">Loading history…</p>
					{:then commits}
						<CommitList commits={commits ?? []} onselect={(c) => selectedCommit = c} />
						{#if selectedCommit}
							{#await getGitCommit({ sha: selectedCommit.sha })}
								<p class="muted">Loading commit…</p>
							{:then detail}
								<div class="commit-detail">
									<div class="commit-detail-header">
										<span class="commit-sha-full">{selectedCommit.sha.slice(0, 12)}</span>
										<span class="commit-detail-msg">{selectedCommit.commit.message.split('\n')[0]}</span>
									</div>
									<div class="commit-detail-meta">
										{selectedCommit.commit.author.name} · {new Date(selectedCommit.commit.author.date).toLocaleString()}
									</div>
									{#if detail?.files}
										{#each detail.files as f}
											<div class="patch-file">
												<div class="patch-filename">{f.filename} <span class="patch-status">{f.status}</span></div>
												{#if f.patch}
													<pre class="patch-content">{f.patch}</pre>
												{/if}
											</div>
										{/each}
									{/if}
								</div>
							{/await}
						{/if}
					{/await}
				{/if}
			</div>

		{:else if rootView === 'commits' && !path}
			<!-- COMMIT LIST -->
			{#await getGitCommits({ ref })}
				<p class="muted">Loading commits…</p>
			{:then commits}
				<CommitList commits={commits ?? []} onselect={(c) => selectedCommit = c} />
				{#if selectedCommit}
					{#await getGitCommit({ sha: selectedCommit.sha })}
						<p class="muted">Loading commit…</p>
					{:then detail}
						<div class="commit-detail">
							<div class="commit-detail-header">
								<span class="commit-sha-full">{selectedCommit.sha.slice(0, 12)}</span>
								<span class="commit-detail-msg">{selectedCommit.commit.message.split('\n')[0]}</span>
							</div>
							<div class="commit-detail-meta">
								{selectedCommit.commit.author.name} · {new Date(selectedCommit.commit.author.date).toLocaleString()}
							</div>
							{#if detail?.files}
								{#each detail.files as f}
									<div class="patch-file">
										<div class="patch-filename">{f.filename} <span class="patch-status">{f.status}</span></div>
										{#if f.patch}
											<pre class="patch-content">{f.patch}</pre>
										{/if}
									</div>
								{/each}
							{/if}
						</div>
					{/await}
				{/if}
			{/await}

		{:else}
			<!-- DIRECTORY LISTING -->
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
										<button class="entry-btn" onclick={() => openFile(entry)}>
											<span class="icon">📄</span>{entry.name}
										</button>
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
		{/if}
	</div>
{:catch}
	<p class="muted">Could not load repository — it may not be initialized yet.</p>
{/await}

<style>
	.git-page { display: flex; flex-direction: column; gap: 0.75rem; }
	.toolbar { display: flex; justify-content: space-between; align-items: center; gap: 1rem; padding-bottom: 0.5rem; border-bottom: 1px solid var(--color-border); }
	.toolbar-right { display: flex; align-items: center; gap: 0.5rem; }
	.breadcrumb { display: flex; align-items: center; gap: 0.2rem; font-family: var(--font-mono); font-size: 13px; flex-wrap: wrap; }
	.crumb { background: none; border: none; color: var(--color-accent); cursor: pointer; padding: 0 0.1rem; font-family: var(--font-mono); font-size: 13px; }
	.crumb:hover { text-decoration: underline; }
	.crumb-static { color: var(--color-text); font-family: var(--font-mono); font-size: 13px; padding: 0 0.1rem; }
	.back-btn { display: flex; align-items: center; gap: 0.25rem; }
	.sep { color: var(--color-text-muted); }
	.ref-select { padding: 0.3rem 0.5rem; border: 1px solid var(--color-border); border-radius: var(--radius); background: var(--color-surface-2); color: var(--color-text); font-size: 13px; font-family: var(--font-mono); }
	.btn-sm { padding: 0.2rem 0.5rem; border: 1px solid var(--color-border); border-radius: 4px; background: none; color: var(--color-text); font-size: 12px; cursor: pointer; }
	.btn-sm.active { background: var(--color-surface-2); border-color: var(--color-accent); color: var(--color-accent); }
	.file-table { width: 100%; border-collapse: collapse; }
	.file-table th { text-align: left; font-size: 11px; text-transform: uppercase; color: var(--color-text-muted); font-weight: 500; padding: 0.25rem 0.5rem; }
	.file-table td { padding: 0.3rem 0.5rem; font-size: 13px; border-top: 1px solid var(--color-border); }
	.entry-btn { background: none; border: none; cursor: pointer; padding: 0; font-size: 13px; display: flex; align-items: center; gap: 0.4rem; color: var(--color-text); }
	.entry-btn:hover { text-decoration: underline; }
	.entry-btn.dir { color: var(--color-accent); }
	.icon { font-size: 14px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.empty { text-align: center; padding: 2rem 0; }

	/* File viewer */
	.file-viewer { display: flex; flex-direction: column; gap: 0; }
	.file-tabs { display: flex; gap: 0; border-bottom: 1px solid var(--color-border); margin-bottom: 0.75rem; }
	.tab { background: none; border: none; border-bottom: 2px solid transparent; padding: 0.4rem 0.75rem; font-size: 13px; color: var(--color-text-muted); cursor: pointer; margin-bottom: -1px; }
	.tab:hover { color: var(--color-text); }
	.tab.active { color: var(--color-accent); border-bottom-color: var(--color-accent); }

	/* Commit detail */
	.commit-detail { margin-top: 1rem; border: 1px solid var(--color-border); border-radius: var(--radius); overflow: hidden; }
	.commit-detail-header { display: flex; align-items: baseline; gap: 0.75rem; padding: 0.75rem 1rem 0.25rem; }
	.commit-sha-full { font-family: var(--font-mono); font-size: 12px; color: var(--color-accent); flex-shrink: 0; }
	.commit-detail-msg { font-size: 14px; font-weight: 500; }
	.commit-detail-meta { padding: 0 1rem 0.75rem; font-size: 12px; color: var(--color-text-muted); border-bottom: 1px solid var(--color-border); }
	.patch-file { border-top: 1px solid var(--color-border); }
	.patch-file:first-of-type { border-top: none; }
	.patch-filename { padding: 0.4rem 1rem; font-family: var(--font-mono); font-size: 12px; background: var(--color-surface-2); display: flex; align-items: center; gap: 0.5rem; }
	.patch-status { font-size: 11px; color: var(--color-text-muted); text-transform: uppercase; }
	.patch-content { margin: 0; padding: 0.75rem 1rem; font-family: var(--font-mono); font-size: 12px; line-height: 1.5; overflow-x: auto; white-space: pre; background: var(--color-surface); }
</style>
