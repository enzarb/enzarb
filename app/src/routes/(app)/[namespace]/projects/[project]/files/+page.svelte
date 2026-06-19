<script lang="ts">
	import { getGitTree } from '$lib/remote/files.remote';
	import { getAgentToken } from '$lib/remote/projects.remote';

	const agentBase = 'https://enzarb.dev/agent';

	type FileEntry = { name: string; path: string; kind: string; size?: number; modified?: string };

	let activeTab: 'working' | 'git' = $state('working');
	let workingFiles: FileEntry[] = $state([]);
	let currentPath: string = $state('');
	let loading = $state(false);
	let uploadInput: HTMLInputElement | undefined = $state();

	async function fetchFiles(token: string, path = '') {
		loading = true;
		try {
			const res = await fetch(`${agentBase}/files?path=${encodeURIComponent(path)}`, {
				headers: { Authorization: `Bearer ${token}` }
			});
			if (res.ok) { workingFiles = await res.json(); currentPath = path; }
		} finally { loading = false; }
	}

	async function downloadFile(token: string, path: string, name: string) {
		const res = await fetch(`${agentBase}/files/download?path=${encodeURIComponent(path)}`, {
			headers: { Authorization: `Bearer ${token}` }
		});
		const blob = await res.blob();
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url; a.download = name; a.click();
		URL.revokeObjectURL(url);
	}

	async function uploadFile(token: string, e: Event) {
		const file = (e.target as HTMLInputElement).files?.[0];
		if (!file) return;
		const uploadPath = currentPath ? `${currentPath}/${file.name}` : file.name;
		await fetch(`${agentBase}/files/upload?path=${encodeURIComponent(uploadPath)}`, {
			method: 'POST', headers: { Authorization: `Bearer ${token}` }, body: file
		});
		await fetchFiles(token, currentPath);
	}

	function formatSize(bytes?: number) {
		if (!bytes) return '';
		if (bytes < 1024) return `${bytes}B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`;
		return `${(bytes / 1024 / 1024).toFixed(1)}MB`;
	}
</script>

{#await Promise.all([getAgentToken(), getGitTree({ path: '', ref: 'main' })]) then [agentToken, gitTree]}
	<div class="files-page">
		<div class="tab-bar">
			<button class="tab {activeTab === 'working' ? 'active' : ''}" onclick={() => { activeTab = 'working'; fetchFiles(agentToken); }}>Working Directory</button>
			<button class="tab {activeTab === 'git' ? 'active' : ''}" onclick={() => (activeTab = 'git')}>Git Repository</button>
		</div>

		{#if activeTab === 'working'}
			<div class="toolbar">
				<div class="breadcrumb">
					<button class="crumb" onclick={() => fetchFiles(agentToken, '')}>~</button>
					{#each currentPath.split('/').filter(Boolean) as part, i}
						<span>/</span>
						<button class="crumb" onclick={() => fetchFiles(agentToken, currentPath.split('/').slice(0, i + 1).join('/'))}>{part}</button>
					{/each}
				</div>
				<div class="toolbar-actions">
					<input type="file" bind:this={uploadInput} onchange={(e) => uploadFile(agentToken, e)} style="display:none" />
					<button class="btn" onclick={() => uploadInput?.click()}>Upload</button>
				</div>
			</div>
			{#if !agentToken}
				<p class="muted">Agent not available — project may still be provisioning.</p>
			{:else if loading}
				<p class="muted">Loading…</p>
			{:else}
				<table class="file-table">
					<thead><tr><th>Name</th><th>Size</th><th>Modified</th><th></th></tr></thead>
					<tbody>
						{#each workingFiles as f}
							<tr>
								<td>
									{#if f.kind === 'dir'}
										<button class="file-link dir" onclick={() => fetchFiles(agentToken, f.path)}>📁 {f.name}</button>
									{:else}
										<span class="file-link">📄 {f.name}</span>
									{/if}
								</td>
								<td class="muted">{formatSize(f.size)}</td>
								<td class="muted">{f.modified ? new Date(f.modified).toLocaleDateString() : ''}</td>
								<td>
									{#if f.kind === 'file'}
										<button class="btn" onclick={() => downloadFile(agentToken, f.path, f.name)}>Download</button>
									{/if}
								</td>
							</tr>
						{:else}
							<tr><td colspan="4" class="muted">Empty directory</td></tr>
						{/each}
					</tbody>
				</table>
			{/if}
		{:else}
			<table class="file-table">
				<thead><tr><th>Name</th><th>Type</th></tr></thead>
				<tbody>
					{#each Array.isArray(gitTree) ? gitTree : [] as entry}
						<tr>
							<td>{entry.type === 'dir' ? '📁' : '📄'} {entry.name}</td>
							<td class="muted">{entry.type}</td>
						</tr>
					{:else}
						<tr><td colspan="2" class="muted">No files or repository not yet initialized.</td></tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</div>
{/await}

<style>
	.files-page { display: flex; flex-direction: column; gap: 1rem; }
	.tab-bar { display: flex; gap: 0; border-bottom: 1px solid var(--color-border); }
	.tab { padding: 0.5rem 1rem; background: none; border: none; border-bottom: 2px solid transparent; color: var(--color-text-muted); font-size: 13px; margin-bottom: -1px; cursor: pointer; }
	.tab.active { color: var(--color-text); border-bottom-color: var(--color-accent); }
	.toolbar { display: flex; justify-content: space-between; align-items: center; }
	.breadcrumb { display: flex; align-items: center; gap: 0.25rem; font-family: var(--font-mono); font-size: 13px; }
	.crumb { background: none; border: none; color: var(--color-accent); cursor: pointer; padding: 0; font-family: var(--font-mono); font-size: 13px; }
	.file-table { width: 100%; }
	.file-link { background: none; border: none; cursor: pointer; color: var(--color-text); font-size: 13px; padding: 0; }
	.file-link.dir { color: var(--color-accent); }
	.muted { color: var(--color-text-muted); }
	.toolbar-actions { display: flex; gap: 0.5rem; }
</style>
