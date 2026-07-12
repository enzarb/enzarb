<script lang="ts">
	import { getAgentAuthToken } from '$lib/agentToken';
	import { getProjectColor } from './layout';
	import FileTreeNode from './FileTreeNode.svelte';

	interface Props {
		agentBase: string;
		namespace: string;
		project: string;
		/** Global (cross-project) mode shows a project selector for the browser. */
		global?: boolean;
		orgProjects?: Record<string, { slug: string; displayName: string }[]>;
		onSelectProject?: (namespace: string, project: string) => void;
		collapsed: boolean;
		onToggleCollapse: () => void;
		onOpenFile: (path: string, label: string) => void;
	}

	let { agentBase, namespace, project, global = false, orgProjects, onSelectProject, collapsed, onToggleCollapse, onOpenFile }: Props = $props();

	const projectOptions = $derived(
		Object.entries(orgProjects ?? {}).flatMap(([ns, projects]) =>
			projects.map((p) => ({ namespace: ns, project: p.slug, displayName: p.displayName }))
		)
	);

	function pickProject(value: string) {
		const [ns, ...rest] = value.split('/');
		onSelectProject?.(ns, rest.join('/'));
	}

	type GitEntry = { path: string; index: string; worktree: string };
	type FileEntry = { name: string; path: string; kind: string };

	let roots: FileEntry[] = $state([]);
	let gitStatus: Record<string, GitEntry> = $state({});
	let authHeader = $state('');
	let loading = $state(true);
	let err = $state('');

	function gitStatusChar(entry: GitEntry | undefined): string | undefined {
		if (!entry) return undefined;
		const i = entry.index;
		const w = entry.worktree;
		if (i === '?' && w === '?') return '??';
		return `${i}${w}`;
	}

	async function loadRoot() {
		loading = true;
		err = '';
		try {
			const token = await getAgentAuthToken(namespace, project);
			if (!token) { err = 'Not authenticated.'; return; }
			authHeader = token;
			const auth = { Authorization: `Bearer ${token}` };

			const [dirRes, gitRes] = await Promise.all([
				fetch(`${agentBase}/files?path=`, { headers: auth }),
				fetch(`${agentBase}/files/git-status`, { headers: auth })
			]);
			if (dirRes.ok) {
				const raw: FileEntry[] = await dirRes.json();
				roots = [
					...raw.filter(e => e.kind === 'dir').sort((a, b) => a.name.localeCompare(b.name)),
					...raw.filter(e => e.kind !== 'dir').sort((a, b) => a.name.localeCompare(b.name))
				];
			}
			if (gitRes.ok) {
				const entries: GitEntry[] = await gitRes.json();
				const map: Record<string, GitEntry> = {};
				for (const e of entries) map[e.path] = e;
				gitStatus = map;
			}
		} catch {
			err = 'Failed to load files.';
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		if (agentBase) loadRoot();
	});
</script>

<div class="file-sidebar" class:collapsed>
	<div class="sidebar-header">
		{#if !collapsed}
			<span class="sidebar-title">Files</span>
			<button class="sidebar-action" onclick={loadRoot} title="Refresh">↺</button>
		{/if}
		<button class="collapse-btn" onclick={onToggleCollapse} title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}>
			{collapsed ? '›' : '‹'}
		</button>
	</div>

	{#if !collapsed && global && projectOptions.length > 0}
		<label class="project-select-row">
			{#if project}
				<span class="project-swatch" style="background: {getProjectColor(namespace, project)}"></span>
			{/if}
			<select value={project ? `${namespace}/${project}` : ''} onchange={(e) => pickProject(e.currentTarget.value)}>
				{#if !project}<option value="" disabled>Choose a project…</option>{/if}
				{#each projectOptions as opt}
					<option value="{opt.namespace}/{opt.project}">{opt.namespace} / {opt.displayName}</option>
				{/each}
			</select>
		</label>
	{/if}

	{#if !collapsed}
		<div class="tree-scroll">
			{#if !agentBase}
				<p class="muted">Select a project to browse files.</p>
			{:else if loading}
				<p class="muted">Loading…</p>
			{:else if err}
				<p class="err">{err}</p>
			{:else if roots.length === 0}
				<p class="muted">Empty workspace.</p>
			{:else}
				{#each roots as entry}
					<FileTreeNode
						name={entry.name}
						path={entry.path}
						kind={entry.kind as 'file' | 'dir'}
						gitStatus={gitStatusChar(gitStatus[entry.path])}
						{agentBase}
						authHeader={`Bearer ${authHeader}`}
						depth={0}
						{onOpenFile}
					/>
				{/each}
			{/if}
		</div>
	{/if}
</div>

<style>
	.file-sidebar { display: flex; flex-direction: column; height: 100%; overflow: hidden; border-right: 1px solid var(--color-border); background: var(--color-surface); min-width: 0; transition: width 0.15s ease; }
	.file-sidebar.collapsed { width: 28px !important; flex-shrink: 0; }
	.sidebar-header { display: flex; align-items: center; gap: 0.25rem; padding: 0.35rem 0.4rem; border-bottom: 1px solid var(--color-border); background: var(--color-surface-2); flex-shrink: 0; min-height: 30px; }
	.sidebar-title { font-size: 11px; text-transform: uppercase; letter-spacing: 0.05em; color: var(--color-text-muted); flex: 1; }
	.sidebar-action { background: none; border: none; color: var(--color-text-muted); cursor: pointer; font-size: 13px; padding: 0.1rem 0.25rem; border-radius: 3px; }
	.sidebar-action:hover { color: var(--color-text); background: var(--color-surface); }
	.collapse-btn { background: none; border: none; color: var(--color-text-muted); cursor: pointer; font-size: 14px; padding: 0.1rem 0.25rem; border-radius: 3px; flex-shrink: 0; line-height: 1; }
	.collapse-btn:hover { color: var(--color-text); background: var(--color-surface); }
	.project-select-row { display: flex; align-items: center; gap: 0.35rem; padding: 0.35rem 0.4rem; border-bottom: 1px solid var(--color-border); flex-shrink: 0; }
	.project-select-row select { flex: 1; font-size: 12px; padding: 0.2rem 0.3rem; }
	.project-swatch { width: 9px; height: 9px; border-radius: 50%; flex-shrink: 0; }
	.tree-scroll { flex: 1; overflow-y: auto; overflow-x: hidden; padding: 0.25rem 0; }
	.muted { color: var(--color-text-muted); font-size: 12px; padding: 0.5rem 0.75rem; }
	.err { color: var(--color-danger); font-size: 12px; padding: 0.5rem 0.75rem; }
</style>
