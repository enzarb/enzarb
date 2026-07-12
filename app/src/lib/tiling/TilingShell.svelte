<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import type { TilingLayout, PaneNode, Tab, LeafPane } from './layout';
	import {
		loadLayout,
		saveLayout,
		loadGlobalLayout,
		saveGlobalLayout,
		stampTabs,
		collectTabs,
		mapPaneLeaves,
		filterTabs,
		countLeaves
	} from './layout';
	import TilingRegion from './TilingRegion.svelte';
	import TilingSplitHandle from './TilingSplitHandle.svelte';
	import FileSidebar from './FileSidebar.svelte';
	import { getAgentAuthToken } from '$lib/agentToken';
	import { getProjectByRef } from '$lib/remote/projects.remote';

	type ProjectRef = { namespace: string; project: string };

	interface Props {
		/** Seed project for single-project mode; omitted in global mode. */
		namespace?: string;
		project?: string;
		/** Cross-project mode: tabs from any project, persisted per-user. */
		global?: boolean;
		/** org slug -> projects, for the new-pane project picker (global mode). */
		orgProjects?: Record<string, { slug: string; displayName: string }[]>;
	}

	let { namespace, project, global = false, orgProjects }: Props = $props();

	// Resolved agentBase per project, keyed "ns/proj". Each tab resolves its own
	// agentBase from this map via getAgentBase(); ensureAgentBase() populates it.
	let projectAgentBases = $state<Record<string, string>>({});
	// In-flight/settled project fetches, deduped so distinct projects are fetched
	// once regardless of how many tabs reference them. Not reactive.
	const projectFetches = new Map<string, Promise<string>>();

	// The project whose files the left-region browser shows. Single-project mode
	// pins it to the seed project; global mode defaults to the first project it
	// finds and can be switched by the user.
	let fileBrowserRef = $state<ProjectRef | null>(
		untrack(() => (!global && namespace && project ? { namespace, project } : null))
	);

	function refKey(ns: string, proj: string) {
		return `${ns}/${proj}`;
	}

	function getAgentBase(ns: string, proj: string): string {
		return projectAgentBases[refKey(ns, proj)] ?? '';
	}

	// Fetch (once) the project object and derive its agentBase from status.agentPath.
	async function ensureAgentBase(ns: string, proj: string): Promise<string> {
		const key = refKey(ns, proj);
		const existing = projectFetches.get(key);
		if (existing) return existing;
		const p = getProjectByRef({ namespace: ns, project: proj })
			.then((proj2: any) => {
				const path = proj2?.status?.agentPath;
				const base = path ? `https://enzarb.dev${path}` : '';
				if (base) projectAgentBases = { ...projectAgentBases, [key]: base };
				return base;
			})
			.catch(() => '');
		projectFetches.set(key, p);
		return p;
	}

	const fileBrowserAgentBase = $derived(
		fileBrowserRef ? getAgentBase(fileBrowserRef.namespace, fileBrowserRef.project) : ''
	);

	let layout = $state<TilingLayout | null>(null);
	let restorationBanner = $state('');
	let bannerDismissed = $state(false);

	// Drag state
	let dragging = $state(false);
	let dragSource = $state<{ paneId: string; tabIndex: number; region: 'left' | 'right' } | null>(null);

	// Pane ID counter — simple incrementing IDs assigned at split time
	let nextPaneId = 0;
	function newPaneId() { return `p${nextPaneId++}`; }

	// ID map: maps string pane IDs to positions in the pane tree
	// We use a flat approach: every leaf is accessed by traversal using its ID stored as __id
	function withId(node: PaneNode, id: string): PaneNode {
		return Object.assign({}, node, { __id: id });
	}

	function assignIds(node: PaneNode): PaneNode {
		if (node.type === 'leaf') return withId(node, newPaneId());
		return {
			...node,
			children: [assignIds(node.children[0]), assignIds(node.children[1])]
		};
	}

	function getLeafId(node: PaneNode): string {
		return (node as any).__id ?? '';
	}

	// Find a leaf by ID
	function findLeaf(node: PaneNode, id: string): LeafPane | null {
		if (node.type === 'leaf') return getLeafId(node) === id ? node : null;
		return findLeaf(node.children[0], id) ?? findLeaf(node.children[1], id);
	}

	// Update a leaf by ID
	function updateLeaf(node: PaneNode, id: string, fn: (leaf: LeafPane) => LeafPane): PaneNode {
		if (node.type === 'leaf') {
			if (getLeafId(node) === id) return withId(fn(node), id);
			return node;
		}
		return {
			...node,
			children: [updateLeaf(node.children[0], id, fn), updateLeaf(node.children[1], id, fn)]
		};
	}

	// Split a leaf by ID
	function splitLeafById(node: PaneNode, id: string, direction: 'h' | 'v', side: 'before' | 'after'): PaneNode {
		if (node.type === 'leaf') {
			if (getLeafId(node) !== id) return node;
			const newLeaf = withId({ type: 'leaf', tabs: [], activeTab: 0 } as LeafPane, newPaneId());
			return {
				type: 'split',
				direction,
				ratio: 0.5,
				children: side === 'after' ? [node, newLeaf] : [newLeaf, node]
			};
		}
		return {
			...node,
			children: [splitLeafById(node.children[0], id, direction, side), splitLeafById(node.children[1], id, direction, side)]
		};
	}

	// Remove a leaf by ID (collapse its sibling up)
	function removeLeaf(node: PaneNode, id: string): PaneNode | null {
		if (node.type === 'leaf') return getLeafId(node) === id ? null : node;
		const left = removeLeaf(node.children[0], id);
		const right = removeLeaf(node.children[1], id);
		if (!left && !right) return null;
		if (!left) return right;
		if (!right) return left;
		return { ...node, children: [left, right] };
	}

	onMount(() => {
		initTilingShell();
	});

	async function initTilingShell() {
		// Load the layout for this mode. Legacy project layouts predate the
		// per-tab namespace/project fields, so stamp them with the seed project.
		let raw: TilingLayout;
		if (global) {
			raw = loadGlobalLayout();
		} else {
			raw = loadLayout(namespace!, project!);
			raw.left.panes = stampTabs(raw.left.panes, namespace!, project!);
			raw.right.panes = stampTabs(raw.right.panes, namespace!, project!);
		}

		// Distinct projects referenced by the layout (plus the seed project so its
		// file browser works even with no tabs open yet).
		const allTabs = [...collectTabs(raw.left.panes), ...collectTabs(raw.right.panes)];
		const projectSet = new Map<string, ProjectRef>();
		for (const t of allTabs) {
			if (t.namespace && t.project) projectSet.set(refKey(t.namespace, t.project), { namespace: t.namespace, project: t.project });
		}
		if (!global && namespace && project) projectSet.set(refKey(namespace, project), { namespace, project });

		// In global mode, default the file browser to the first project we know of.
		if (!fileBrowserRef) {
			const first = projectSet.values().next().value as ProjectRef | undefined;
			if (first) fileBrowserRef = first;
			else if (orgProjects) {
				for (const [ns, projects] of Object.entries(orgProjects)) {
					if (projects.length > 0) { fileBrowserRef = { namespace: ns, project: projects[0].slug }; break; }
				}
			}
		}

		// Resolve agentBase for every distinct project up front.
		await Promise.all([...projectSet.values()].map((r) => ensureAgentBase(r.namespace, r.project)));

		// Validate terminal/agent tabs against each project's live resources. Ids
		// are only unique within a project, so validity is judged per project. A
		// project we couldn't reach keeps its tabs (no live set to compare against).
		const validByProject = new Map<string, Set<string>>();
		await Promise.all(
			[...projectSet.values()].map(async (r) => {
				const base = getAgentBase(r.namespace, r.project);
				if (!base) return;
				const token = await getAgentAuthToken(r.namespace, r.project);
				if (!token) return;
				const auth = { Authorization: `Bearer ${token}` };
				try {
					const [procs, sessions] = await Promise.all([
						fetch(`${base}/processes`, { headers: auth }).then((res) => (res.ok ? res.json() : [])),
						fetch(`${base}/agent/sessions`, { headers: auth }).then((res) => (res.ok ? res.json() : []))
					]);
					validByProject.set(
						refKey(r.namespace, r.project),
						new Set([...procs.map((p: any) => p.id), ...sessions.map((s: any) => s.id)])
					);
				} catch {}
			})
		);

		let dropped = 0;
		const keep = (t: Tab) => {
			if (t.kind !== 'terminal' && t.kind !== 'agent') return true;
			const valid = validByProject.get(refKey(t.namespace, t.project));
			if (!valid) return true; // project unreachable — keep, don't discard
			const ok = valid.has(t.id);
			if (!ok) dropped++;
			return ok;
		};
		raw.left.panes = filterTabs(raw.left.panes, keep);
		raw.right.panes = filterTabs(raw.right.panes, keep);

		// Assign IDs to all pane leaves
		raw.left.panes = assignIds(raw.left.panes);
		raw.right.panes = assignIds(raw.right.panes);

		layout = raw;
		if (dropped > 0) {
			restorationBanner = `${dropped} tab${dropped > 1 ? 's' : ''} could not be restored (process or session no longer exists).`;
		}
	}

	function save() {
		if (!layout) return;
		if (global) saveGlobalLayout(layout);
		else saveLayout(namespace!, project!, layout);
	}

	function handleTabClose(region: 'left' | 'right', paneId: string, tabIndex: number) {
		if (!layout) return;
		const updated = updateLeaf(layout[region].panes, paneId, (leaf) => {
			const tabs = leaf.tabs.filter((_, i) => i !== tabIndex);
			const activeTab = Math.min(leaf.activeTab, Math.max(0, tabs.length - 1));
			return { ...leaf, tabs, activeTab };
		});
		const closedLeaf = findLeaf(updated, paneId);
		// If that was the pane's last tab and it's part of a split, close the
		// pane itself (collapsing its sibling) rather than leaving an empty
		// picker behind. A lone, unsplit pane has nowhere to collapse to, so
		// it keeps showing the picker.
		if (closedLeaf && closedLeaf.tabs.length === 0 && countLeaves(updated) > 1) {
			layout[region].panes = removeLeaf(updated, paneId) ?? updated;
		} else {
			layout[region].panes = updated;
		}
		save();
	}

	function handleTabSelect(region: 'left' | 'right', paneId: string, tabIndex: number) {
		if (!layout) return;
		layout[region].panes = updateLeaf(layout[region].panes, paneId, (leaf) => ({ ...leaf, activeTab: tabIndex }));
		save();
	}

	function handleSplit(region: 'left' | 'right', paneId: string, direction: 'h' | 'v', side: 'before' | 'after') {
		if (!layout) return;
		layout[region].panes = splitLeafById(layout[region].panes, paneId, direction, side);
		save();
	}

	function handleAddTab(region: 'left' | 'right', paneId: string, tab: Tab) {
		if (!layout) return;
		layout[region].panes = updateLeaf(layout[region].panes, paneId, (leaf) => {
			const existingIndex = leaf.tabs.findIndex((t) => t.kind === tab.kind && t.id === tab.id && t.namespace === tab.namespace && t.project === tab.project);
			if (existingIndex !== -1) {
				return { ...leaf, activeTab: existingIndex };
			}
			return { ...leaf, tabs: [...leaf.tabs, tab], activeTab: leaf.tabs.length };
		});
		save();
	}

	function handleTabDragStart(region: 'left' | 'right', paneId: string, tabIndex: number) {
		dragging = true;
		dragSource = { paneId, tabIndex, region };
	}

	function handleTabDrop(region: 'left' | 'right', targetPaneId: string, zone: 'top' | 'bottom' | 'left' | 'right' | 'center') {
		if (!layout || !dragSource || dragSource.region !== region) {
			dragging = false;
			dragSource = null;
			return;
		}

		const sourcePaneId = dragSource.paneId;
		const sourceTabIndex = dragSource.tabIndex;
		const sourceLeaf = findLeaf(layout[region].panes, sourcePaneId);
		if (!sourceLeaf) { dragging = false; dragSource = null; return; }

		const tab = sourceLeaf.tabs[sourceTabIndex];
		if (!tab) { dragging = false; dragSource = null; return; }

		// Remove from source
		layout[region].panes = updateLeaf(layout[region].panes, sourcePaneId, (leaf) => {
			const tabs = leaf.tabs.filter((_, i) => i !== sourceTabIndex);
			return { ...leaf, tabs, activeTab: Math.min(leaf.activeTab, Math.max(0, tabs.length - 1)) };
		});

		if (zone === 'center') {
			// Move to target pane
			layout[region].panes = updateLeaf(layout[region].panes, targetPaneId, (leaf) => {
				const existingIndex = leaf.tabs.findIndex((t) => t.kind === tab.kind && t.id === tab.id && t.namespace === tab.namespace && t.project === tab.project);
				if (existingIndex !== -1) {
					return { ...leaf, activeTab: existingIndex };
				}
				return { ...leaf, tabs: [...leaf.tabs, tab], activeTab: leaf.tabs.length };
			});
		} else {
			// Split target pane and add tab to new pane
			const direction = zone === 'left' || zone === 'right' ? 'h' : 'v';
			const side = zone === 'right' || zone === 'bottom' ? 'after' : 'before';
			layout[region].panes = splitLeafById(layout[region].panes, targetPaneId, direction, side);
			// Find the new leaf (it has empty tabs) and add the tab
			layout[region].panes = mapPaneLeaves(layout[region].panes, (leaf) => {
				if ((leaf as any).__id !== targetPaneId && leaf.tabs.length === 0) {
					return { ...leaf, tabs: [tab], activeTab: 0 };
				}
				return leaf;
			});
		}

		dragging = false;
		dragSource = null;
		save();
	}

	// nodeId is the path string built by TilingRegion (e.g. "left-0-1"); currentId
	// tracks the path of `node` as we recurse so we only touch the matching split.
	function updateSplitRatio(node: PaneNode, nodeId: string, ratio: number, currentId: string): PaneNode {
		if (node.type === 'leaf') return node;
		if (currentId === nodeId) return { ...node, ratio };
		return {
			...node,
			children: [
				updateSplitRatio(node.children[0], nodeId, ratio, `${currentId}-0`),
				updateSplitRatio(node.children[1], nodeId, ratio, `${currentId}-1`)
			]
		};
	}

	// The main divider's meaning depends on whether a file viewer is showing:
	// with no file open it's the file browser's own edge; with one open, the
	// browser's width (layout.divider) stays put and this only resizes the
	// viewer, so closing the file always restores the divider beside the browser.
	function handleMainDividerDrag(delta: number) {
		if (!layout) return;
		if (hasOpenFile) {
			const viewerFraction = layout.left.sidebar?.viewerFraction ?? 0.3;
			if (!layout.left.sidebar) layout.left.sidebar = { viewerFraction: 0.3, collapsed: false };
			layout.left.sidebar.viewerFraction = Math.max(0.1, Math.min(0.7, viewerFraction + delta));
		} else {
			layout.divider = Math.max(0.1, Math.min(0.9, layout.divider + delta));
		}
		save();
	}

	// Drag between the file browser and the file viewer: reapportions the
	// combined width between them without moving the outer main divider.
	function handleSidebarWidthDrag(delta: number) {
		if (!layout) return;
		const viewerFraction = layout.left.sidebar?.viewerFraction ?? 0.3;
		const combined = layout.divider + viewerFraction;
		const deltaOfMain = delta * combined;
		const newDivider = Math.max(0.05, Math.min(combined - 0.05, layout.divider + deltaOfMain));
		if (!layout.left.sidebar) layout.left.sidebar = { viewerFraction: combined - newDivider, collapsed: false };
		layout.left.sidebar.viewerFraction = combined - newDivider;
		layout.divider = newDivider;
		save();
	}

	function toggleSidebar() {
		if (!layout) return;
		if (!layout.left.sidebar) layout.left.sidebar = { viewerFraction: 0.3, collapsed: false };
		layout.left.sidebar.collapsed = !layout.left.sidebar.collapsed;
		save();
	}

	function openFileInLeft(path: string, label: string) {
		if (!layout || !fileBrowserRef) return;
		const ns = fileBrowserRef.namespace;
		const proj = fileBrowserRef.project;
		// Find first leaf in left region
		const firstLeaf = findFirstLeaf(layout.left.panes);
		if (!firstLeaf) return;
		const leafId = getLeafId(firstLeaf);
		// Check if already open (same file, same project)
		const existing = firstLeaf.tabs.findIndex(
			(t) => t.kind === 'file' && t.id === path && t.namespace === ns && t.project === proj
		);
		if (existing >= 0) {
			handleTabSelect('left', leafId, existing);
			return;
		}
		handleAddTab('left', leafId, { kind: 'file', id: path, label, namespace: ns, project: proj });
	}

	function findFirstLeaf(node: PaneNode): LeafPane | null {
		if (node.type === 'leaf') return node;
		return findFirstLeaf(node.children[0]) ?? findFirstLeaf(node.children[1]);
	}

	const sidebarCollapsed = $derived(layout?.left.sidebar?.collapsed ?? false);
	const hasOpenFile = $derived(layout ? collectTabs(layout.left.panes).length > 0 : false);
	const viewerFraction = $derived(layout?.left.sidebar?.viewerFraction ?? 0.3);
	// The file browser's own share of the screen never changes across open/close —
	// only the extra viewer width is added on top while a file is open.
	const leftWidth = $derived(
		layout ? `${(layout.divider + (hasOpenFile ? viewerFraction : 0)) * 100}%` : '25%'
	);
	const sidebarWidth = $derived(
		layout && hasOpenFile ? `${(layout.divider / (layout.divider + viewerFraction)) * 100}%` : '100%'
	);
</script>

<div
	class="tiling-shell"
	role="presentation"
	onmouseup={() => { if (dragging) { dragging = false; dragSource = null; } }}
>
	{#if restorationBanner && !bannerDismissed}
		<div class="restoration-banner">
			<span>{restorationBanner}</span>
			<button onclick={() => bannerDismissed = true}>×</button>
		</div>
	{/if}

	{#if layout}
		<div class="main-area">
			<!-- Left region: collapses to a thin toggle strip when the file browser is closed,
			     so there's only one draggable split (the main divider) while it's collapsed. -->
			<div class="left-region" style="width: {sidebarCollapsed ? '28px' : leftWidth}; flex-shrink: 0;">
				<div class="left-inner">
					<!-- File sidebar -->
					<div class="sidebar-wrap" style="width: {sidebarCollapsed ? '28px' : sidebarWidth}; flex-shrink: 0;">
						<FileSidebar
							agentBase={fileBrowserAgentBase}
							namespace={fileBrowserRef?.namespace ?? ''}
							project={fileBrowserRef?.project ?? ''}
							{global}
							{orgProjects}
							onSelectProject={(ns, proj) => {
								fileBrowserRef = { namespace: ns, project: proj };
								ensureAgentBase(ns, proj);
							}}
							collapsed={sidebarCollapsed}
							onToggleCollapse={toggleSidebar}
							onOpenFile={openFileInLeft}
						/>
					</div>
					{#if !sidebarCollapsed && hasOpenFile}
						<TilingSplitHandle
							direction="h"
							onDrag={(delta) => handleSidebarWidthDrag(delta)}
						/>
						<!-- Left pane area (file viewer) -->
						<div class="pane-area" style="flex: 1; min-width: 0; overflow: hidden;">
							<TilingRegion
								node={layout.left.panes}
								nodeId="left"
								regionKind="left"
								{getAgentBase}
								{ensureAgentBase}
								{global}
								{orgProjects}
								defaultRef={fileBrowserRef}
								{dragging}
								{dragSource}
								onUpdate={() => {}}
								onTabClose={(id, idx) => handleTabClose('left', id, idx)}
								onTabSelect={(id, idx) => handleTabSelect('left', id, idx)}
								onSplit={(id, dir, side) => handleSplit('left', id, dir, side)}
								onAddTab={(id, tab) => handleAddTab('left', id, tab)}
								onTabDragStart={(id, idx) => handleTabDragStart('left', id, idx)}
								onTabDrop={(id, zone) => handleTabDrop('left', id, zone)}
								onRatioChange={(id, ratio) => {
									if (!layout) return;
									layout.left.panes = updateSplitRatio(layout.left.panes, id, ratio, 'left');
									save();
								}}
							/>
						</div>
					{/if}
				</div>
			</div>

			<!-- Main divider: only shown while the file browser is open, so there's a single
			     top-level split when it's closed. -->
			{#if !sidebarCollapsed}
				<TilingSplitHandle direction="h" onDrag={(delta) => handleMainDividerDrag(delta)} />
			{/if}

			<!-- Right region -->
			<div class="right-region" style="flex: 1; min-width: 0; overflow: hidden;">
				<TilingRegion
					node={layout.right.panes}
					nodeId="right"
					regionKind="right"
					{getAgentBase}
					{ensureAgentBase}
					{global}
					{orgProjects}
					defaultRef={!global && namespace && project ? { namespace, project } : fileBrowserRef}
					{dragging}
					{dragSource}
					onUpdate={() => {}}
					onTabClose={(id, idx) => handleTabClose('right', id, idx)}
					onTabSelect={(id, idx) => handleTabSelect('right', id, idx)}
					onSplit={(id, dir, side) => handleSplit('right', id, dir, side)}
					onAddTab={(id, tab) => handleAddTab('right', id, tab)}
					onTabDragStart={(id, idx) => handleTabDragStart('right', id, idx)}
					onTabDrop={(id, zone) => handleTabDrop('right', id, zone)}
					onRatioChange={(id, ratio) => {
						if (!layout) return;
						layout.right.panes = updateSplitRatio(layout.right.panes, id, ratio, 'right');
						save();
					}}
				/>
			</div>
		</div>
	{:else}
		<div class="loading-shell">
			<div class="spinner"></div>
			<p>Loading workspace…</p>
		</div>
	{/if}
</div>

<style>
	.tiling-shell { display: flex; flex-direction: column; height: 100%; overflow: hidden; }
	.restoration-banner { display: flex; align-items: center; justify-content: space-between; padding: 0.4rem 0.75rem; background: #2a1a00; border-bottom: 1px solid #6a4500; font-size: 12px; color: #f5a623; flex-shrink: 0; }
	.restoration-banner button { background: none; border: none; color: inherit; cursor: pointer; font-size: 16px; line-height: 1; opacity: 0.7; }
	.restoration-banner button:hover { opacity: 1; }
	.main-area { display: flex; flex-direction: row; flex: 1; overflow: hidden; min-height: 0; }
	.left-region { display: flex; flex-direction: column; overflow: hidden; border-right: none; min-height: 0; }
	.left-inner { display: flex; flex-direction: row; height: 100%; overflow: hidden; }
	.sidebar-wrap { overflow: hidden; min-height: 0; display: flex; flex-direction: column; }
	.pane-area { display: flex; overflow: hidden; height: 100%; }
	.right-region { display: flex; overflow: hidden; }
	.loading-shell { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 0.75rem; }
	.loading-shell p { font-size: 13px; color: var(--color-text-muted); }
	.spinner { width: 24px; height: 24px; border: 2px solid var(--color-border); border-top-color: var(--color-accent); border-radius: 50%; animation: spin 0.8s linear infinite; }
	@keyframes spin { to { transform: rotate(360deg); } }
</style>
