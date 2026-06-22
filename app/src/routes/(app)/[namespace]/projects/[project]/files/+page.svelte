<script lang="ts">
	import { getAgentToken, getProject } from '$lib/remote/projects.remote';
	import CodeViewer from '$lib/components/CodeViewer.svelte';
	import { onMount } from 'svelte';

	type FileEntry = { name: string; path: string; kind: string; size?: number; modified?: string };

	let agentBase = $state('');
	let token = $state('');
	let files = $state<FileEntry[]>([]);
	let currentPath = $state('');
	let loading = $state(false);
	let ready = $state(false);
	let error = $state('');
	let uploadInput: HTMLInputElement | undefined = $state();

	// File viewer state
	let viewFile = $state<{ path: string; name: string } | null>(null);
	let viewContent = $state('');
	let viewLoading = $state(false);
	let viewError = $state('');
	let viewImageUrl = $state('');

	const IMAGE_EXTS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'ico', 'bmp']);
	const BINARY_EXTS = new Set(['wasm', 'pdf', 'zip', 'tar', 'gz', 'bz2', 'xz', 'bin', 'exe', 'dll', 'so', 'dylib', 'class', 'pyc']);

	function fileKind(name: string): 'text' | 'image' | 'binary' {
		const ext = name.split('.').pop()?.toLowerCase() ?? '';
		if (IMAGE_EXTS.has(ext)) return 'image';
		if (BINARY_EXTS.has(ext)) return 'binary';
		return 'text';
	}

	async function init() {
		const [agentToken, project] = await Promise.all([getAgentToken(), getProject()]);
		const path = project?.status?.agentPath;
		if (!path) { error = 'Agent not ready — project may still be provisioning.'; return; }
		token = agentToken;
		agentBase = `https://enzarb.dev${path}`;
		ready = true;
		await cd('');
	}

	async function cd(path: string) {
		if (!agentBase) return;
		loading = true;
		error = '';
		try {
			const res = await fetch(`${agentBase}/files?path=${encodeURIComponent(path)}`, {
				headers: { Authorization: `Bearer ${token}` }
			});
			if (!res.ok) { error = `Failed to list directory (${res.status})`; return; }
			const entries: FileEntry[] = await res.json();
			files = [
				...entries.filter(e => e.kind === 'dir').sort((a, b) => a.name.localeCompare(b.name)),
				...entries.filter(e => e.kind !== 'dir').sort((a, b) => a.name.localeCompare(b.name))
			];
			currentPath = path;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Unknown error';
		} finally {
			loading = false;
		}
	}

	function parentPath() {
		const parts = currentPath.split('/').filter(Boolean);
		return parts.slice(0, -1).join('/');
	}

	async function download(path: string, name: string) {
		const res = await fetch(`${agentBase}/files/download?path=${encodeURIComponent(path)}`, {
			headers: { Authorization: `Bearer ${token}` }
		});
		const blob = await res.blob();
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url; a.download = name; a.click();
		URL.revokeObjectURL(url);
	}

	async function openFile(entry: FileEntry) {
		const kind = fileKind(entry.name);
		if (kind === 'binary') {
			download(entry.path, entry.name);
			return;
		}
		viewFile = { path: entry.path, name: entry.name };
		viewContent = '';
		viewImageUrl = '';
		viewError = '';
		viewLoading = true;
		try {
			const res = await fetch(`${agentBase}/files/download?path=${encodeURIComponent(entry.path)}`, {
				headers: { Authorization: `Bearer ${token}` }
			});
			if (!res.ok) throw new Error(`HTTP ${res.status}`);
			if (kind === 'image') {
				const blob = await res.blob();
				viewImageUrl = URL.createObjectURL(blob);
			} else {
				viewContent = await res.text();
			}
		} catch (e) {
			viewError = e instanceof Error ? e.message : 'Failed to load file';
		} finally {
			viewLoading = false;
		}
	}

	function closeViewer() {
		if (viewImageUrl) URL.revokeObjectURL(viewImageUrl);
		viewFile = null;
		viewContent = '';
		viewImageUrl = '';
		viewError = '';
	}

	async function upload(e: Event) {
		const file = (e.target as HTMLInputElement).files?.[0];
		if (!file) return;
		const dest = currentPath ? `${currentPath}/${file.name}` : file.name;
		await fetch(`${agentBase}/files/upload?path=${encodeURIComponent(dest)}`, {
			method: 'POST', headers: { Authorization: `Bearer ${token}` }, body: file
		});
		await cd(currentPath);
	}

	function fmtSize(b?: number) {
		if (!b) return '';
		if (b < 1024) return `${b} B`;
		if (b < 1048576) return `${(b / 1024).toFixed(1)} KB`;
		return `${(b / 1048576).toFixed(1)} MB`;
	}

	function fmtDate(s?: string) {
		return s ? new Date(s).toLocaleDateString() : '';
	}

	const breadcrumbs = $derived(
		currentPath ? currentPath.split('/').filter(Boolean) : []
	);

	onMount(() => { init(); });
</script>

{#if error && !ready}
	<p class="muted">{error}</p>
{:else if !ready}
	<p class="muted">Loading…</p>
{:else}
	<div class="files-page">
		<div class="toolbar">
			<nav class="breadcrumb" aria-label="path">
				{#if viewFile}
					<button class="crumb back-btn" onclick={closeViewer}>← Back</button>
					<span class="sep">/</span>
					<span class="crumb-static">{viewFile.name}</span>
				{:else}
					<button class="crumb" onclick={() => cd('')}>~</button>
					{#each breadcrumbs as part, i}
						<span class="sep">/</span>
						<button
							class="crumb"
							onclick={() => cd(breadcrumbs.slice(0, i + 1).join('/'))}
						>{part}</button>
					{/each}
				{/if}
			</nav>
			<div class="toolbar-right">
				{#if error}<span class="err">{error}</span>{/if}
				{#if viewFile}
					<button class="btn-sm" onclick={() => download(viewFile!.path, viewFile!.name)}>Download</button>
				{:else}
					<input type="file" bind:this={uploadInput} onchange={upload} style="display:none" />
					<button class="btn" onclick={() => uploadInput?.click()}>Upload</button>
				{/if}
			</div>
		</div>

		{#if viewFile}
			{#if viewError}
				<p class="muted">{viewError}</p>
			{:else if viewImageUrl}
				<div class="image-wrap">
					<img src={viewImageUrl} alt={viewFile.name} class="image-preview" />
				</div>
			{:else}
				<CodeViewer content={viewContent} filename={viewFile.name} loading={viewLoading} />
			{/if}
		{:else if loading}
			<p class="muted">Loading…</p>
		{:else}
			<table class="file-table">
				<thead>
					<tr><th>Name</th><th>Size</th><th>Modified</th><th></th></tr>
				</thead>
				<tbody>
					{#if currentPath}
						<tr>
							<td colspan="4">
								<button class="entry-btn" onclick={() => cd(parentPath())}>
									<span class="icon">⬆</span> ..
								</button>
							</td>
						</tr>
					{/if}
					{#each files as f}
						<tr>
							<td>
								{#if f.kind === 'dir'}
									<button class="entry-btn dir" onclick={() => cd(f.path)}>
										<span class="icon">📁</span>{f.name}
									</button>
								{:else}
									<button class="entry-btn" onclick={() => openFile(f)}>
										<span class="icon">📄</span>{f.name}
									</button>
								{/if}
							</td>
							<td class="muted">{fmtSize(f.size)}</td>
							<td class="muted">{fmtDate(f.modified)}</td>
							<td>
								{#if f.kind === 'file'}
									<button class="btn-sm" onclick={() => download(f.path, f.name)}>Download</button>
								{/if}
							</td>
						</tr>
					{:else}
						<tr><td colspan="4" class="muted empty">Empty directory</td></tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</div>
{/if}

<style>
	.files-page { display: flex; flex-direction: column; gap: 0.75rem; }
	.toolbar { display: flex; justify-content: space-between; align-items: center; gap: 1rem; padding-bottom: 0.5rem; border-bottom: 1px solid var(--color-border); }
	.toolbar-right { display: flex; align-items: center; gap: 0.5rem; }
	.breadcrumb { display: flex; align-items: center; gap: 0.2rem; font-family: var(--font-mono); font-size: 13px; flex-wrap: wrap; }
	.crumb { background: none; border: none; color: var(--color-accent); cursor: pointer; padding: 0 0.1rem; font-family: var(--font-mono); font-size: 13px; }
	.crumb:hover { text-decoration: underline; }
	.crumb-static { color: var(--color-text); font-family: var(--font-mono); font-size: 13px; padding: 0 0.1rem; }
	.back-btn { display: flex; align-items: center; gap: 0.25rem; }
	.sep { color: var(--color-text-muted); }
	.file-table { width: 100%; border-collapse: collapse; }
	.file-table th { text-align: left; font-size: 11px; text-transform: uppercase; color: var(--color-text-muted); font-weight: 500; padding: 0.25rem 0.5rem; }
	.file-table td { padding: 0.3rem 0.5rem; font-size: 13px; border-top: 1px solid var(--color-border); }
	.entry-btn { background: none; border: none; cursor: pointer; padding: 0; font-size: 13px; display: flex; align-items: center; gap: 0.4rem; color: var(--color-text); }
	.entry-btn:hover { text-decoration: underline; }
	.entry-btn.dir { color: var(--color-accent); }
	.icon { font-size: 14px; }
	.btn-sm { padding: 0.2rem 0.5rem; border: 1px solid var(--color-border); border-radius: 4px; background: none; color: var(--color-text); font-size: 12px; cursor: pointer; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.empty { text-align: center; padding: 2rem 0; }
	.err { color: var(--color-danger, #c0392b); font-size: 12px; }
	.image-wrap { display: flex; justify-content: flex-start; padding: 1rem 0; }
	.image-preview { max-width: 100%; border: 1px solid var(--color-border); border-radius: var(--radius); }
</style>
