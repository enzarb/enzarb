<script lang="ts">
	import type { LeafPane, Tab } from './layout';
	import { getProjectColor } from './layout';
	import type { Component } from 'svelte';
	import AgentPane from '$lib/agent/AgentPane.svelte';
	import NewPaneDialog from './NewPaneDialog.svelte';
	import { getAgentAuthToken } from '$lib/agentToken';

	// xterm.js and shiki (syntax highlighting) are, respectively, the two
	// largest dependencies in the app bundle; load their host components on
	// demand instead of shipping both to every user regardless of whether
	// they ever open a terminal or a text file.
	type TerminalPaneComponent = Component<{
		agentBase: string;
		namespace: string;
		project: string;
		processId: string;
	}>;
	let TerminalPane: TerminalPaneComponent | null = $state(null);

	type CodeViewerComponent = Component<{ content: string; filename: string }>;
	let CodeViewer: CodeViewerComponent | null = $state(null);

	type ProjectRef = { namespace: string; project: string };

	interface Props {
		pane: LeafPane;
		paneId: string;
		regionKind: 'left' | 'right';
		getAgentBase: (ns: string, proj: string) => string;
		ensureAgentBase: (ns: string, proj: string) => Promise<string>;
		global: boolean;
		orgProjects?: Record<string, { slug: string; displayName: string }[]>;
		defaultRef: ProjectRef | null;
		onTabClose: (paneId: string, tabIndex: number) => void;
		onTabSelect: (paneId: string, tabIndex: number) => void;
		onSplit: (paneId: string, direction: 'h' | 'v', side: 'before' | 'after') => void;
		onAddTab: (paneId: string, tab: Tab) => void;
		onTabDragStart: (paneId: string, tabIndex: number) => void;
		onTabDrop: (targetPaneId: string, zone: DropZone) => void;
		dragging: boolean;
	}

	type DropZone = 'top' | 'bottom' | 'left' | 'right' | 'center';

	let { pane, paneId, regionKind, getAgentBase, ensureAgentBase, global, orgProjects, defaultRef, onTabClose, onTabSelect, onSplit, onAddTab, onTabDragStart, onTabDrop, dragging }: Props = $props();

	let hoverZone = $state<DropZone | null>(null);
	let el: HTMLDivElement | undefined = $state();
	let addMenuOpen = $state(false);

	// File viewer state (for left region)
	type FileContent = { type: 'loading' } | { type: 'text'; content: string; path: string } | { type: 'image'; dataUrl: string } | { type: 'binary' } | { type: 'error'; message: string };
	let fileContent = $state<FileContent>({ type: 'loading' });

	const activeTab = $derived(pane.tabs[pane.activeTab] ?? null);
	const hasTerminalTab = $derived(pane.tabs.some((t) => t.kind === 'terminal'));

	$effect(() => {
		if (hasTerminalTab && !TerminalPane) {
			import('$lib/terminal/TerminalPane.svelte').then((m) => {
				TerminalPane = m.default as TerminalPaneComponent;
			});
		}
	});

	$effect(() => {
		if (activeTab?.kind === 'file' && !CodeViewer) {
			import('$lib/components/CodeViewer.svelte').then((m) => {
				CodeViewer = m.default as CodeViewerComponent;
			});
		}
	});

	$effect(() => {
		if (activeTab?.kind === 'file') {
			loadFile(activeTab);
		}
	});

	async function loadFile(tab: Tab) {
		const path = tab.id;
		const agentBase = getAgentBase(tab.namespace, tab.project);
		fileContent = { type: 'loading' };
		try {
			const token = await getAgentAuthToken(tab.namespace, tab.project);
			if (!token) { fileContent = { type: 'error', message: 'Not authenticated.' }; return; }
			const auth = { Authorization: `Bearer ${token}` };
			const ext = path.split('.').pop()?.toLowerCase() ?? '';
			const IMAGE_EXTS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'ico', 'bmp']);
			const BINARY_EXTS = new Set(['wasm', 'pdf', 'zip', 'tar', 'gz', 'bz2', 'xz', 'bin', 'exe', 'dll', 'so', 'dylib', 'class', 'pyc']);
			const MIME: Record<string, string> = { svg: 'image/svg+xml', webp: 'image/webp', gif: 'image/gif', png: 'image/png', bmp: 'image/bmp', ico: 'image/x-icon' };

			if (BINARY_EXTS.has(ext)) { fileContent = { type: 'binary' }; return; }

			if (IMAGE_EXTS.has(ext)) {
				const res = await fetch(`${agentBase}/files/download?path=${encodeURIComponent(path)}`, { headers: auth });
				if (!res.ok) { fileContent = { type: 'error', message: `HTTP ${res.status}` }; return; }
				const ab = await res.arrayBuffer();
				const bytes = new Uint8Array(ab);
				let binary = '';
				for (let i = 0; i < bytes.byteLength; i++) binary += String.fromCharCode(bytes[i]);
				fileContent = { type: 'image', dataUrl: `data:${MIME[ext] ?? 'image/jpeg'};base64,${btoa(binary)}` };
				return;
			}

			const res = await fetch(`${agentBase}/files/download?path=${encodeURIComponent(path)}`, { headers: auth });
			if (!res.ok) { fileContent = { type: 'error', message: `HTTP ${res.status}` }; return; }
			const text = await res.text();
			fileContent = { type: 'text', content: text, path };
		} catch {
			fileContent = { type: 'error', message: 'Failed to load file.' };
		}
	}

	function onMouseMove(e: MouseEvent) {
		if (!dragging || !el) return;
		const rect = el.getBoundingClientRect();
		const x = e.clientX - rect.left;
		const y = e.clientY - rect.top;
		const w = rect.width;
		const h = rect.height;
		const edgeX = w * 0.25;
		const edgeY = h * 0.25;
		if (x < edgeX) hoverZone = 'left';
		else if (x > w - edgeX) hoverZone = 'right';
		else if (y < edgeY) hoverZone = 'top';
		else if (y > h - edgeY) hoverZone = 'bottom';
		else hoverZone = 'center';
	}

	function onMouseLeave() {
		hoverZone = null;
	}

	function onDrop() {
		if (!dragging || !hoverZone) return;
		onTabDrop(paneId, hoverZone);
		hoverZone = null;
	}

	function handleAddTab(tab: Tab) {
		onAddTab(paneId, tab);
		addMenuOpen = false;
	}
</script>

<!--
	role="region" is a genuine landmark (each pane is independently navigable),
	not a workaround — removing it would regress AT navigation. The mouse
	handlers only track cursor position for tab drag-and-drop reordering, a
	supplementary mouse-only interaction with no meaningful keyboard
	equivalent; the actual interactive controls (tabs, below) are all
	keyboard-accessible on their own.
-->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
	bind:this={el}
	class="tiling-pane"
	onmousemove={onMouseMove}
	onmouseleave={onMouseLeave}
	onmouseup={onDrop}
	role="region"
>
	<div class="tab-bar">
		<div class="tabs">
			{#each pane.tabs as tab, i}
				<div
					class="tab"
					class:active={i === pane.activeTab}
					role="button"
					tabindex="0"
					title={`${tab.label ?? tab.id} — ${tab.namespace}/${tab.project}`}
					style="--project-color: {getProjectColor(tab.namespace, tab.project)}"
					onclick={() => onTabSelect(paneId, i)}
					onkeydown={(e) => e.key === 'Enter' && onTabSelect(paneId, i)}
					onmousedown={() => onTabDragStart(paneId, i)}
				>
					<span class="tab-swatch" title={`${tab.namespace}/${tab.project}`}></span>
					<span class="tab-icon">{tab.kind === 'terminal' ? '⌨' : tab.kind === 'agent' ? '◆' : '📄'}</span>
					<span class="tab-name">{tab.label ?? tab.id}</span>
					<button
						class="tab-close"
						onmousedown={(e) => e.stopPropagation()}
						onclick={(e) => { e.stopPropagation(); onTabClose(paneId, i); }}
					>×</button>
				</div>
			{/each}
		</div>
		<div class="tab-actions">
			{#if regionKind === 'right' && pane.tabs.length > 0}
				<div class="add-menu-wrap">
					<button class="action-btn" title="Open another terminal or agent session" onclick={() => addMenuOpen = !addMenuOpen}>
						<svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5">
							<line x1="7" y1="2" x2="7" y2="12" />
							<line x1="2" y1="7" x2="12" y2="7" />
						</svg>
					</button>
					{#if addMenuOpen}
						<button
							type="button"
							class="add-menu-backdrop"
							aria-label="Close menu"
							onclick={() => addMenuOpen = false}
						></button>
						<div class="add-menu-popover">
							<NewPaneDialog
								{getAgentBase}
								{ensureAgentBase}
								{global}
								{orgProjects}
								{defaultRef}
								{regionKind}
								onOpenTerminal={(id, label, ns, proj) => handleAddTab({ kind: 'terminal', id, label, namespace: ns, project: proj })}
								onOpenAgent={(id, label, ns, proj) => handleAddTab({ kind: 'agent', id, label, namespace: ns, project: proj })}
								onCreateTerminal={(id, label, ns, proj) => handleAddTab({ kind: 'terminal', id, label, namespace: ns, project: proj })}
								onCreateAgent={(id, label, ns, proj) => handleAddTab({ kind: 'agent', id, label, namespace: ns, project: proj })}
							/>
						</div>
					{/if}
				</div>
			{/if}
			<button class="action-btn" title="Split side by side" onclick={() => onSplit(paneId, 'h', 'after')}>
				<svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.3">
					<rect x="1" y="1" width="12" height="12" rx="1.5" />
					<line x1="7" y1="1" x2="7" y2="13" />
				</svg>
			</button>
			<button class="action-btn" title="Split top and bottom" onclick={() => onSplit(paneId, 'v', 'after')}>
				<svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.3">
					<rect x="1" y="1" width="12" height="12" rx="1.5" />
					<line x1="1" y1="7" x2="13" y2="7" />
				</svg>
			</button>
		</div>
	</div>

	<div class="pane-content">
		{#if pane.tabs.length === 0 && regionKind === 'left'}
			<div class="empty-file-pane">
				<p class="file-msg">No file open — select one from the file browser.</p>
			</div>
		{:else if pane.tabs.length === 0}
			<NewPaneDialog
				{getAgentBase}
				{ensureAgentBase}
				{global}
				{orgProjects}
				{defaultRef}
				{regionKind}
				onOpenTerminal={(id, label, ns, proj) => handleAddTab({ kind: 'terminal', id, label, namespace: ns, project: proj })}
				onOpenAgent={(id, label, ns, proj) => handleAddTab({ kind: 'agent', id, label, namespace: ns, project: proj })}
				onCreateTerminal={(id, label, ns, proj) => handleAddTab({ kind: 'terminal', id, label, namespace: ns, project: proj })}
				onCreateAgent={(id, label, ns, proj) => handleAddTab({ kind: 'agent', id, label, namespace: ns, project: proj })}
			/>
		{:else if activeTab}
			<!--
				Terminal/agent tabs stay mounted even when not active so their
				socket + session state survive tab switches; we just hide the
				inactive ones. File tabs are cheap to re-render, so only the
				active one is shown.
			-->
			{#each pane.tabs as tab (tab.kind + ':' + tab.id)}
				{#if tab.kind === 'terminal' || tab.kind === 'agent'}
					<div class="tab-content" class:hidden={tab !== activeTab}>
						{#if tab.kind === 'terminal'}
							{#if TerminalPane}
								<TerminalPane agentBase={getAgentBase(tab.namespace, tab.project)} namespace={tab.namespace} project={tab.project} processId={tab.id} />
							{/if}
						{:else}
							<AgentPane agentBase={getAgentBase(tab.namespace, tab.project)} namespace={tab.namespace} project={tab.project} sessionId={tab.id} />
						{/if}
					</div>
				{/if}
			{/each}
			{#if activeTab.kind === 'file'}
				<div class="file-viewer">
					{#if fileContent.type === 'loading'}
						<p class="file-msg">Loading…</p>
					{:else if fileContent.type === 'error'}
						<p class="file-msg err">{fileContent.message}</p>
					{:else if fileContent.type === 'binary'}
						<p class="file-msg">Binary file — <a href="{getAgentBase(activeTab.namespace, activeTab.project)}/files/download?path={encodeURIComponent(activeTab.id)}" target="_blank">download</a></p>
					{:else if fileContent.type === 'image'}
						<div class="image-container">
							<img src={fileContent.dataUrl} alt={activeTab.label ?? activeTab.id} />
						</div>
					{:else if fileContent.type === 'text'}
						{#if CodeViewer}
							<CodeViewer content={fileContent.content} filename={fileContent.path} />
						{/if}
					{/if}
				</div>
			{/if}
		{/if}

		{#if dragging && hoverZone}
			<div class="drop-overlay">
				<div class="drop-zone top" class:active={hoverZone === 'top'}></div>
				<div class="drop-zone bottom" class:active={hoverZone === 'bottom'}></div>
				<div class="drop-zone left" class:active={hoverZone === 'left'}></div>
				<div class="drop-zone right" class:active={hoverZone === 'right'}></div>
				<div class="drop-zone center" class:active={hoverZone === 'center'}></div>
			</div>
		{/if}
	</div>
</div>

<style>
	.tiling-pane { display: flex; flex-direction: column; flex: 1 1 auto; width: 100%; height: 100%; overflow: hidden; position: relative; min-width: 0; min-height: 0; }
	.tab-bar { display: flex; align-items: stretch; border-bottom: 1px solid var(--color-border); background: var(--color-surface-2); flex-shrink: 0; }
	.tabs { flex: 1; display: flex; align-items: stretch; overflow-x: auto; }
	.tab { display: flex; align-items: center; gap: 0.3rem; padding: 0 0.4rem 0 0.5rem; border: none; border-right: 1px solid var(--color-border); border-left: 3px solid var(--project-color, transparent); background: none; color: var(--color-text-muted); font-size: 12px; cursor: pointer; white-space: nowrap; user-select: none; }
	.tab:hover { background: var(--color-surface); color: var(--color-text); }
	.tab.active { background: var(--color-surface); color: var(--color-text); box-shadow: inset 0 2px 0 var(--project-color, var(--color-accent)); }
	.tab-swatch { width: 7px; height: 7px; border-radius: 50%; background: var(--project-color, var(--color-text-muted)); flex-shrink: 0; }
	.tab-icon { font-size: 10px; flex-shrink: 0; }
	.tab-name { overflow: hidden; text-overflow: ellipsis; max-width: 140px; font-family: var(--font-mono); }
	.tab-close { background: none; border: none; color: var(--color-text-muted); cursor: pointer; font-size: 14px; line-height: 1; padding: 0 0.1rem; border-radius: 3px; }
	.tab-close:hover { color: var(--color-danger); }
	.tab-actions { display: flex; align-items: center; flex-shrink: 0; border-left: 1px solid var(--color-border); }
	.action-btn { width: 28px; height: 28px; border: none; background: none; color: var(--color-text-muted); cursor: pointer; font-size: 11px; display: flex; align-items: center; justify-content: center; }
	.action-btn:hover { background: var(--color-surface); color: var(--color-accent); }
	.add-menu-wrap { position: relative; display: flex; }
	.add-menu-backdrop { position: fixed; inset: 0; z-index: 29; background: transparent; border: none; padding: 0; cursor: default; }
	.add-menu-popover { position: absolute; top: 100%; right: 0; z-index: 30; width: 300px; max-height: 420px; overflow-y: auto; background: var(--color-surface); border: 1px solid var(--color-border); border-radius: 6px; box-shadow: var(--shadow); }
	.pane-content { flex: 1; overflow: hidden; position: relative; display: flex; flex-direction: column; min-height: 0; }
	.tab-content { flex: 1; min-height: 0; display: flex; flex-direction: column; }
	.tab-content.hidden { display: none; }
	.file-viewer { flex: 1; overflow: auto; display: flex; flex-direction: column; min-height: 0; }
	.empty-file-pane { flex: 1; display: flex; align-items: center; justify-content: center; }
	.file-msg { color: var(--color-text-muted); font-size: 13px; padding: 1rem; }
	.file-msg.err { color: var(--color-danger); }
	.image-container { flex: 1; display: flex; align-items: center; justify-content: center; padding: 1rem; overflow: auto; }
	.image-container img { max-width: 100%; max-height: 100%; object-fit: contain; }

	/* Drop overlay zones */
	.drop-overlay { position: absolute; inset: 0; pointer-events: none; z-index: 20; }
	.drop-zone { position: absolute; background: color-mix(in srgb, var(--color-accent) 20%, transparent); border: 2px solid transparent; transition: background 0.1s; }
	.drop-zone.active { background: color-mix(in srgb, var(--color-accent) 35%, transparent); border-color: var(--color-accent); }
	.drop-zone.top { inset: 0 0 75% 0; }
	.drop-zone.bottom { inset: 75% 0 0 0; }
	.drop-zone.left { inset: 25% auto 25% 0; width: 25%; }
	.drop-zone.right { inset: 25% 0 25% auto; width: 25%; }
	.drop-zone.center { inset: 25% 25% 25% 25%; border-radius: 4px; }
</style>
