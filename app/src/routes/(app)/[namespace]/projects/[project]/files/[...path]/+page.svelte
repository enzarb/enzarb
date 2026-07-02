<script lang="ts">
	import { page } from '$app/state';
	import { getProject } from '$lib/remote/projects.remote';
	import { getAgentAuthToken } from '$lib/agentToken';
	import CodeViewer from '$lib/components/CodeViewer.svelte';

	type FileEntry = { name: string; path: string; kind: string; size?: number; modified?: string };
	type GitEntry = { path: string; index: string; worktree: string };

	const IMAGE_EXTS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'ico', 'bmp']);
	const BINARY_EXTS = new Set(['wasm', 'pdf', 'zip', 'tar', 'gz', 'bz2', 'xz', 'bin', 'exe', 'dll', 'so', 'dylib', 'class', 'pyc']);
	const MIME: Record<string, string> = { svg: 'image/svg+xml', webp: 'image/webp', gif: 'image/gif', png: 'image/png', bmp: 'image/bmp', ico: 'image/x-icon' };

	function fileExt(name: string) { return name.split('.').pop()?.toLowerCase() ?? ''; }

	let refreshKey = $state(0);

	const filesBase = $derived(
		page.url.pathname.slice(0, page.url.pathname.indexOf('/files') + '/files'.length)
	);
	const breadcrumbs = $derived((page.params.path ?? '').split('/').filter(Boolean));
	const parentUrl = $derived.by(() => {
		const parts = (page.params.path ?? '').split('/').filter(Boolean);
		const parent = parts.slice(0, -1).join('/');
		return parent ? `${filesBase}/${parent}` : filesBase;
	});

	const dataPromise = $derived(load(page.params.path ?? '', refreshKey));

	async function load(path: string, _refresh: number) {
		const [agentToken, project] = await Promise.all([getAgentAuthToken(), getProject()]);
		const agentPath = project?.status?.agentPath;
		if (!agentPath) return { type: 'error' as const, message: 'Agent not ready — project may still be provisioning.', gitStatus: {} as Record<string, GitEntry>, agentBase: '' };

		const agentBase = `https://enzarb.dev${agentPath}`;
		const auth = { Authorization: `Bearer ${agentToken}` };

		const gitStatus: Record<string, GitEntry> = {};
		try {
			const gs = await fetch(`${agentBase}/files/git-status`, { headers: auth });
			if (gs.ok) {
				const entries: GitEntry[] = await gs.json();
				for (const e of entries) gitStatus[e.path] = e;
			}
		} catch { /* ignore */ }

		const dirRes = await fetch(`${agentBase}/files?path=${encodeURIComponent(path)}`, { headers: auth });
		if (dirRes.ok) {
			const raw: FileEntry[] = await dirRes.json();
			const entries = [
				...raw.filter(e => e.kind === 'dir').sort((a, b) => a.name.localeCompare(b.name)),
				...raw.filter(e => e.kind !== 'dir').sort((a, b) => a.name.localeCompare(b.name))
			];
			return { type: 'dir' as const, path, entries, gitStatus, agentBase };
		}

		const name = path.split('/').pop() ?? '';
		const ext = fileExt(name);

		if (BINARY_EXTS.has(ext)) {
			return { type: 'binary' as const, path, name, gitStatus, agentBase };
		}

		if (IMAGE_EXTS.has(ext)) {
			const res = await fetch(`${agentBase}/files/download?path=${encodeURIComponent(path)}`, { headers: auth });
			if (!res.ok) return { type: 'error' as const, message: `HTTP ${res.status}`, gitStatus, agentBase };
			const ab = await res.arrayBuffer();
			const bytes = new Uint8Array(ab);
			let binary = '';
			for (let i = 0; i < bytes.byteLength; i++) binary += String.fromCharCode(bytes[i]);
			const dataUrl = `data:${MIME[ext] ?? 'image/jpeg'};base64,${btoa(binary)}`;
			return { type: 'image' as const, path, name, dataUrl, gitStatus, agentBase };
		}

		const hasChanges = path in gitStatus;
		if (hasChanges) {
			const diffRes = await fetch(`${agentBase}/files/git-diff?path=${encodeURIComponent(path)}`, { headers: auth });
			if (diffRes.ok) {
				const content = await diffRes.text();
				return { type: 'diff' as const, path, name, content, gitStatus, agentBase };
			}
		}

		const fileRes = await fetch(`${agentBase}/files/download?path=${encodeURIComponent(path)}`, { headers: auth });
		if (!fileRes.ok) return { type: 'error' as const, message: `HTTP ${fileRes.status}`, gitStatus, agentBase };
		const content = await fileRes.text();
		return { type: 'file' as const, path, name, content, gitStatus, agentBase };
	}

	function gitColorClass(gitStatus: Record<string, GitEntry>, path: string, isDir: boolean): string {
		if (isDir) {
			const prefix = path.endsWith('/') ? path : path + '/';
			for (const [p, e] of Object.entries(gitStatus)) {
				if (p.startsWith(prefix) || p === path) return entryColorClass(e);
			}
			return '';
		}
		const e = gitStatus[path];
		return e ? entryColorClass(e) : '';
	}

	function entryColorClass(e: GitEntry): string {
		if (e.index === '?' && e.worktree === '?') return 'git-untracked';
		if (e.index !== ' ' && e.index !== '?') return 'git-staged';
		if (e.worktree !== ' ' && e.worktree !== '?') return 'git-modified';
		return '';
	}

	function gitLabel(e: GitEntry): string {
		if (e.index === '?' && e.worktree === '?') return 'U';
		const s = e.index !== ' ' && e.index !== '?' ? e.index : e.worktree;
		return s !== ' ' ? s : '?';
	}

	async function download(agentBase: string, path: string, name: string) {
		const fresh = await getAgentAuthToken();
		const res = await fetch(`${agentBase}/files/download?path=${encodeURIComponent(path)}`, {
			headers: { Authorization: `Bearer ${fresh}` }
		});
		const blob = await res.blob();
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url; a.download = name; a.click();
		URL.revokeObjectURL(url);
	}

	let uploadInput: HTMLInputElement | undefined = $state();

	async function upload(e: Event, agentBase: string, dir: string) {
		const file = (e.target as HTMLInputElement).files?.[0];
		if (!file) return;
		const fresh = await getAgentAuthToken();
		const dest = dir ? `${dir}/${file.name}` : file.name;
		await fetch(`${agentBase}/files/upload?path=${encodeURIComponent(dest)}`, {
			method: 'POST', headers: { Authorization: `Bearer ${fresh}` }, body: file
		});
		refreshKey++;
	}

	// Commit dialog
	let commitDialog: HTMLDialogElement | undefined = $state();
	let commitMessage = $state('');
	let committing = $state(false);
	let commitError = $state('');

	function openCommitDialog() {
		commitMessage = '';
		commitError = '';
		commitDialog?.showModal();
	}

	async function doCommit(agentBase: string) {
		if (!commitMessage.trim()) return;
		committing = true;
		commitError = '';
		try {
			const fresh = await getAgentAuthToken();
			const res = await fetch(`${agentBase}/files/git-commit`, {
				method: 'POST',
				headers: { Authorization: `Bearer ${fresh}`, 'Content-Type': 'application/json' },
				body: JSON.stringify({ message: commitMessage.trim() })
			});
			if (!res.ok) {
				commitError = await res.text() || `HTTP ${res.status}`;
				return;
			}
			commitDialog?.close();
			refreshKey++;
		} finally {
			committing = false;
		}
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
</script>

{#await dataPromise}
	<p class="muted">Loading…</p>
{:then data}
	{@const changedFiles = Object.values(data.gitStatus)}
	<div class="layout">
		<div class="main">
			<div class="toolbar">
				<nav class="breadcrumb" aria-label="path">
					{#if data.type === 'dir'}
						<a class="crumb" href={filesBase}>~</a>
						{#each breadcrumbs as part, i}
							<span class="sep">/</span>
							<a class="crumb" href="{filesBase}/{breadcrumbs.slice(0, i + 1).join('/')}">{part}</a>
						{/each}
					{:else}
						<a class="crumb back-btn" href={parentUrl}>← Back</a>
						<span class="sep">/</span>
						<span class="crumb-static">{data.name ?? data.path}</span>
						{#if data.type === 'diff'}<span class="diff-badge">diff</span>{/if}
					{/if}
				</nav>
				<div class="toolbar-right">
					{#if data.type === 'dir'}
						<input type="file" bind:this={uploadInput} onchange={(e) => upload(e, data.agentBase, data.path)} class="hidden" />
						<button class="btn" onclick={() => uploadInput?.click()}>Upload</button>
					{:else if data.type === 'file' || data.type === 'image'}
						<button class="btn-sm" onclick={() => download(data.agentBase, data.path, data.name)}>Download</button>
					{/if}
				</div>
			</div>

			{#if data.type === 'error'}
				<p class="muted">{data.message}</p>
			{:else if data.type === 'binary'}
				<p class="muted">Binary file — <button class="link-btn" onclick={() => download(data.agentBase, data.path, data.name)}>download</button></p>
			{:else if data.type === 'image'}
				<div class="image-wrap">
					<img src={data.dataUrl} alt={data.name} class="image-preview" />
				</div>
			{:else if data.type === 'diff'}
				<div class="diff-view" aria-label="git diff">
					{#each data.content.split('\n') as line}
						{@const cls = line.startsWith('+') && !line.startsWith('+++') ? 'add'
							: line.startsWith('-') && !line.startsWith('---') ? 'del'
							: line.startsWith('@@') ? 'hunk'
							: line.startsWith('diff ') || line.startsWith('index ') || line.startsWith('---') || line.startsWith('+++') ? 'meta'
							: 'ctx'}
						<div class="diff-line {cls}">{line || ' '}</div>
					{/each}
				</div>
			{:else if data.type === 'file'}
				<CodeViewer content={data.content} filename={data.name} loading={false} />
			{:else if data.type === 'dir'}
				<table class="file-table">
					<thead>
						<tr><th>Name</th><th>Size</th><th>Modified</th><th></th></tr>
					</thead>
					<tbody>
						{#if data.path}
							<tr>
								<td colspan="4">
									<a class="entry-btn" href={parentUrl}>
										<span class="icon">⬆</span> ..
									</a>
								</td>
							</tr>
						{/if}
						{#each data.entries as f}
							{@const colorCls = gitColorClass(data.gitStatus, f.path, f.kind === 'dir')}
							<tr>
								<td>
									<a class="entry-btn {colorCls}" href="{filesBase}/{f.path}">
										<span class="icon">{f.kind === 'dir' ? '📁' : '📄'}</span>{f.name}
									</a>
								</td>
								<td class="muted">{fmtSize(f.size)}</td>
								<td class="muted">{fmtDate(f.modified)}</td>
								<td>
									{#if f.kind === 'file'}
										<button class="btn-sm" onclick={() => download(data.agentBase, f.path, f.name)}>Download</button>
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

		{#if changedFiles.length > 0}
			<aside class="git-sidebar">
				<div class="sidebar-header">
					<span class="sidebar-title">Changes</span>
					<button class="commit-btn" onclick={openCommitDialog}>Commit…</button>
				</div>
				<ul class="change-list">
					{#each changedFiles as entry}
						<li>
							<a class="change-entry {entryColorClass(entry)}" href="{filesBase}/{entry.path}" title={entry.path}>
								<span class="change-label">{gitLabel(entry)}</span>
								<span class="change-path">{entry.path}</span>
							</a>
						</li>
					{/each}
				</ul>

				<dialog
					bind:this={commitDialog}
					class="commit-dialog"
					oncancel={(e) => { e.preventDefault(); commitDialog?.close(); }}
				>
					<h3>Commit changes</h3>
					<p class="dialog-sub">{changedFiles.length} file{changedFiles.length !== 1 ? 's' : ''} will be staged and committed.</p>
					<!-- svelte-ignore a11y_autofocus -->
					<textarea
						class="commit-msg"
						placeholder="Commit message…"
						rows={4}
						autofocus
						bind:value={commitMessage}
						onkeydown={(e) => {
							if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) doCommit(data.agentBase);
						}}
					></textarea>
					{#if commitError}
						<p class="commit-error">{commitError}</p>
					{/if}
					<div class="dialog-actions">
						<button class="btn" onclick={() => commitDialog?.close()} disabled={committing}>Cancel</button>
						<button
							class="btn btn-primary"
							onclick={() => doCommit(data.agentBase)}
							disabled={committing || !commitMessage.trim()}
						>
							{committing ? 'Committing…' : 'Commit'}
						</button>
					</div>
				</dialog>
			</aside>
		{/if}
	</div>
{:catch err}
	<p class="muted">{err?.message ?? 'Failed to load'}</p>
{/await}

<style>
	.hidden { display: none; }
	.layout { display: flex; gap: 1rem; align-items: flex-start; }
	.main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 0.75rem; }

	.toolbar { display: flex; justify-content: space-between; align-items: center; gap: 1rem; padding-bottom: 0.5rem; border-bottom: 1px solid var(--color-border); }
	.toolbar-right { display: flex; align-items: center; gap: 0.5rem; }
	.breadcrumb { display: flex; align-items: center; gap: 0.2rem; font-family: var(--font-mono); font-size: 13px; flex-wrap: wrap; }
	.crumb { color: var(--color-text-muted); padding: 0 0.1rem; font-family: var(--font-mono); font-size: 13px; text-decoration: none; }
	.crumb:hover { color: var(--color-text); text-decoration: underline; }
	.crumb-static { color: var(--color-text); font-family: var(--font-mono); font-size: 13px; padding: 0 0.1rem; }
	.back-btn { display: flex; align-items: center; gap: 0.25rem; color: var(--color-text-muted); }
	.sep { color: var(--color-text-muted); }
	.diff-badge { font-size: 10px; font-family: var(--font-mono); padding: 0.1rem 0.35rem; border-radius: 3px; background: rgba(210, 153, 34, 0.2); color: #d29922; border: 1px solid rgba(210, 153, 34, 0.4); margin-left: 0.25rem; }

	.file-table { width: 100%; border-collapse: collapse; }
	.file-table th { text-align: left; font-size: 11px; text-transform: uppercase; color: var(--color-text-muted); font-weight: 500; padding: 0.25rem 0.5rem; }
	.file-table td { padding: 0.3rem 0.5rem; font-size: 13px; border-top: 1px solid var(--color-border); }

	.entry-btn { background: none; border: none; cursor: pointer; padding: 0; font-size: 13px; display: flex; align-items: center; gap: 0.4rem; color: var(--color-text); text-decoration: none; }
	.entry-btn:hover { text-decoration: underline; }
	.entry-btn.git-modified { color: #d29922; }
	.entry-btn.git-staged { color: #3fb950; }
	.entry-btn.git-untracked { color: var(--color-text-muted); }

	.icon { font-size: 14px; }
	.btn-sm { padding: 0.2rem 0.5rem; border: 1px solid var(--color-border); border-radius: 4px; background: none; color: var(--color-text); font-size: 12px; cursor: pointer; }
	.link-btn { background: none; border: none; color: var(--color-text); cursor: pointer; padding: 0; font-size: 13px; text-decoration: underline; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.empty { text-align: center; padding: 2rem 0; }
	.image-wrap { display: flex; justify-content: flex-start; padding: 1rem 0; }
	.image-preview { max-width: 100%; border: 1px solid var(--color-border); border-radius: var(--radius); }

	.diff-view { font-family: var(--font-mono); font-size: 12px; line-height: 1.5; overflow-x: auto; border: 1px solid var(--color-border); border-radius: var(--radius); }
	.diff-line { padding: 0 0.75rem; white-space: pre; min-height: 1.5em; }
	.diff-line.add { background: rgba(46, 160, 67, 0.15); color: #3fb950; }
	.diff-line.del { background: rgba(248, 81, 73, 0.15); color: #f85149; }
	.diff-line.hunk { background: rgba(88, 166, 255, 0.1); color: #58a6ff; }
	.diff-line.meta { color: var(--color-text-muted); }
	.diff-line.ctx { color: var(--color-text); }

	/* Git sidebar */
	.git-sidebar {
		width: 220px;
		flex-shrink: 0;
		border: 1px solid var(--color-border);
		border-radius: var(--radius);
		overflow: hidden;
		font-size: 12px;
	}
	.sidebar-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 0.4rem 0.6rem;
		border-bottom: 1px solid var(--color-border);
		background: var(--color-surface-2, var(--color-surface));
	}
	.sidebar-title {
		font-size: 11px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--color-text-muted);
	}
	.commit-btn {
		padding: 0.15rem 0.5rem;
		border: 1px solid var(--color-border);
		border-radius: 4px;
		background: none;
		color: var(--color-text);
		font-size: 11px;
		cursor: pointer;
	}
	.commit-btn:hover { background: var(--color-surface-2, rgba(255,255,255,0.05)); }
	.change-list { list-style: none; margin: 0; padding: 0.25rem 0; }
	.change-entry {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		padding: 0.2rem 0.6rem;
		text-decoration: none;
		color: var(--color-text);
		overflow: hidden;
	}
	.change-entry:hover { background: var(--color-surface-2, rgba(255,255,255,0.04)); }
	.change-entry.git-modified { color: #d29922; }
	.change-entry.git-staged { color: #3fb950; }
	.change-entry.git-untracked { color: var(--color-text-muted); }
	.change-label {
		font-family: var(--font-mono);
		font-size: 10px;
		flex-shrink: 0;
		width: 12px;
		text-align: center;
	}
	.change-path {
		font-family: var(--font-mono);
		font-size: 11px;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	/* Commit dialog */
	.commit-dialog {
		border: 1px solid var(--color-border);
		border-radius: var(--radius);
		background: var(--color-surface);
		color: var(--color-text);
		padding: 1.25rem;
		width: 400px;
		max-width: calc(100vw - 2rem);
		box-shadow: var(--shadow);
	}
	.commit-dialog::backdrop { background: rgba(0,0,0,0.5); }
	.commit-dialog h3 { margin: 0 0 0.25rem; font-size: 15px; }
	.dialog-sub { margin: 0 0 0.75rem; font-size: 12px; color: var(--color-text-muted); }
	.commit-msg {
		width: 100%;
		box-sizing: border-box;
		padding: 0.4rem 0.5rem;
		font-size: 13px;
		font-family: var(--font-mono);
		background: var(--color-surface-2, var(--color-surface));
		color: var(--color-text);
		border: 1px solid var(--color-border);
		border-radius: var(--radius);
		resize: vertical;
		margin-bottom: 0.75rem;
	}
	.commit-error { margin: 0 0 0.75rem; font-size: 12px; color: var(--color-danger, #c0392b); white-space: pre-wrap; }
	.dialog-actions { display: flex; justify-content: flex-end; gap: 0.5rem; }
</style>
