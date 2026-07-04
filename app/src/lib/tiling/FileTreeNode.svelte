<script lang="ts">
	import FileTreeNode from './FileTreeNode.svelte';

	interface Props {
		name: string;
		path: string;
		kind: 'file' | 'dir';
		gitStatus?: string;
		agentBase: string;
		authHeader: string;
		depth: number;
		onOpenFile: (path: string, label: string) => void;
	}

	let { name, path, kind, gitStatus, agentBase, authHeader, depth, onOpenFile }: Props = $props();

	let expanded = $state(false);
	let children: Array<{ name: string; path: string; kind: string; gitStatus?: string }> = $state([]);
	let loaded = $state(false);
	let loading = $state(false);

	async function toggle() {
		if (kind !== 'dir') {
			onOpenFile(path, name);
			return;
		}
		expanded = !expanded;
		if (expanded && !loaded) {
			loading = true;
			try {
				const res = await fetch(`${agentBase}/files?path=${encodeURIComponent(path)}`, {
					headers: { Authorization: authHeader }
				});
				if (res.ok) {
					const raw = await res.json();
					children = [
						...raw.filter((e: any) => e.kind === 'dir').sort((a: any, b: any) => a.name.localeCompare(b.name)),
						...raw.filter((e: any) => e.kind !== 'dir').sort((a: any, b: any) => a.name.localeCompare(b.name))
					];
					loaded = true;
				}
			} catch {}
			loading = false;
		}
	}

	function gitColor(status: string | undefined): string {
		if (!status) return '';
		if (status.includes('U') || status === '??') return 'git-untracked';
		if (status[0] !== ' ' && status[0] !== '?') return 'git-staged';
		if (status[1] !== ' ' && status[1] !== '?') return 'git-modified';
		return '';
	}

	const colorClass = $derived(gitColor(gitStatus));
	const indent = $derived(depth * 12);
</script>

<div class="tree-node">
	<button
		class="node-row {colorClass}"
		style="padding-left: {indent + 8}px"
		onclick={toggle}
		title={path}
	>
		{#if kind === 'dir'}
			<span class="arrow" class:open={expanded}>{expanded ? '▾' : '▸'}</span>
		{:else}
			<span class="file-indent"></span>
		{/if}
		<span class="node-name">{name}</span>
		{#if loading}
			<span class="loading-dot">…</span>
		{/if}
	</button>
	{#if expanded && loaded}
		{#each children as child}
			<FileTreeNode
				name={child.name}
				path={child.path}
				kind={child.kind as 'file' | 'dir'}
				gitStatus={child.gitStatus}
				{agentBase}
				{authHeader}
				depth={depth + 1}
				{onOpenFile}
			/>
		{/each}
	{/if}
</div>

<style>
	.tree-node { display: flex; flex-direction: column; }
	.node-row { display: flex; align-items: center; gap: 0.3rem; border: none; background: none; color: var(--color-text); font-size: 12px; font-family: var(--font-mono); cursor: pointer; text-align: left; width: 100%; padding-top: 0.2rem; padding-right: 0.5rem; padding-bottom: 0.2rem; min-width: 0; white-space: nowrap; overflow: hidden; }
	.node-row:hover { background: var(--color-surface-2); }
	.arrow { flex-shrink: 0; width: 12px; font-size: 10px; color: var(--color-text-muted); }
	.file-indent { flex-shrink: 0; width: 12px; }
	.node-name { overflow: hidden; text-overflow: ellipsis; flex: 1; }
	.loading-dot { color: var(--color-text-muted); font-size: 10px; flex-shrink: 0; }
	.git-untracked { color: #73c991; }
	.git-staged { color: #4fc1ff; }
	.git-modified { color: #ce9178; }
</style>
