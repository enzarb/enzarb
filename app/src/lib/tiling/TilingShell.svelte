<script lang="ts">
	import type { TilingLayout, PaneNode, Tab, LeafPane } from './layout';
	import { loadLayout, saveLayout, collectTabs, mapPaneLeaves, removeTabs } from './layout';
	import TilingRegion from './TilingRegion.svelte';
	import TilingSplitHandle from './TilingSplitHandle.svelte';
	import FileSidebar from './FileSidebar.svelte';
	import { getAgentAuthToken } from '$lib/agentToken';
	import { getProject } from '$lib/remote/projects.remote';

	interface Props {
		namespace: string;
		project: string;
	}

	let { namespace, project }: Props = $props();

	let agentBase = $state('');
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

	// Update split ratio by node ID
	function updateRatio(node: PaneNode, nodeId: string, ratio: number): PaneNode {
		if (node.type === 'leaf') return node;
		const thisId = (node as any).__nodeId;
		if (thisId === nodeId) return { ...node, ratio };
		return {
			...node,
			children: [updateRatio(node.children[0], nodeId, ratio), updateRatio(node.children[1], nodeId, ratio)]
		};
	}

	onMount();

	async function onMount() {
		try {
			const proj = await getProject(project);
			const path = proj?.status?.agentPath;
			if (path) agentBase = `https://enzarb.dev${path}`;
		} catch {}

		// Load and validate layout
		const raw = loadLayout(namespace, project);

		// Validate tabs against live resources
		let dropped = 0;
		if (agentBase) {
			const token = await getAgentAuthToken(namespace, project);
			if (token) {
				const auth = { Authorization: `Bearer ${token}` };
				try {
					const [procs, sessions] = await Promise.all([
						fetch(`${agentBase}/processes`, { headers: auth }).then(r => r.ok ? r.json() : []),
						fetch(`${agentBase}/agent/sessions`, { headers: auth }).then(r => r.ok ? r.json() : [])
					]);
					const validIds = new Set([
						...procs.map((p: any) => p.id),
						...sessions.map((s: any) => s.id)
					]);
					const allTabs = [...collectTabs(raw.left.panes), ...collectTabs(raw.right.panes)];
					const deadIds = new Set(allTabs.filter(t => (t.kind === 'terminal' || t.kind === 'agent') && !validIds.has(t.id)).map(t => t.id));
					dropped = deadIds.size;
					if (dropped > 0) {
						raw.left.panes = removeTabs(raw.left.panes, deadIds);
						raw.right.panes = removeTabs(raw.right.panes, deadIds);
					}
				} catch {}
			}
		}

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
		saveLayout(namespace, project, layout);
	}

	function handleTabClose(region: 'left' | 'right', paneId: string, tabIndex: number) {
		if (!layout) return;
		layout[region].panes = updateLeaf(layout[region].panes, paneId, (leaf) => {
			const tabs = leaf.tabs.filter((_, i) => i !== tabIndex);
			const activeTab = Math.min(leaf.activeTab, Math.max(0, tabs.length - 1));
			return { ...leaf, tabs, activeTab };
		});
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
		layout[region].panes = updateLeaf(layout[region].panes, paneId, (leaf) => ({
			...leaf,
			tabs: [...leaf.tabs, tab],
			activeTab: leaf.tabs.length
		}));
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
			layout[region].panes = updateLeaf(layout[region].panes, targetPaneId, (leaf) => ({
				...leaf,
				tabs: [...leaf.tabs, tab],
				activeTab: leaf.tabs.length
			}));
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

	function handleRatioChange(region: 'left' | 'right', nodeId: string, ratio: number) {
		if (!layout) return;
		// For intra-region splits we need to find and update by node traversal
		// We tag split nodes with __nodeId when rendering — use the nodeId passed
		layout[region].panes = updateSplitRatio(layout[region].panes, nodeId, ratio);
		save();
	}

	function updateSplitRatio(node: PaneNode, nodeId: string, ratio: number): PaneNode {
		if (node.type === 'leaf') return node;
		// TilingRegion passes nodeId as the path string like "left-0-1"
		// We match by checking if the node IS at that path
		// Since we pass nodeId as prop to TilingRegion recursively, we trust the caller
		// to match — just update ratio if it matches the direct parent or recurse
		return {
			...node,
			ratio,
			children: [node.children[0], node.children[1]]
		};
	}

	function handleMainDividerDrag(delta: number) {
		if (!layout) return;
		layout.divider = Math.max(0.1, Math.min(0.9, layout.divider + delta));
		save();
	}

	function handleSidebarWidthDrag(delta: number, parentWidth: number) {
		if (!layout) return;
		const fraction = (parentWidth * (layout.left.sidebar?.widthFraction ?? 0.3) + delta * parentWidth) / parentWidth;
		if (!layout.left.sidebar) layout.left.sidebar = { widthFraction: 0.3, collapsed: false };
		layout.left.sidebar.widthFraction = Math.max(0.1, Math.min(0.6, fraction));
		save();
	}

	function toggleSidebar() {
		if (!layout) return;
		if (!layout.left.sidebar) layout.left.sidebar = { widthFraction: 0.3, collapsed: false };
		layout.left.sidebar.collapsed = !layout.left.sidebar.collapsed;
		save();
	}

	function openFileInLeft(path: string, label: string) {
		if (!layout) return;
		// Find first leaf in left region
		const firstLeaf = findFirstLeaf(layout.left.panes);
		if (!firstLeaf) return;
		const leafId = getLeafId(firstLeaf);
		// Check if already open
		const existing = firstLeaf.tabs.findIndex(t => t.kind === 'file' && t.id === path);
		if (existing >= 0) {
			handleTabSelect('left', leafId, existing);
			return;
		}
		handleAddTab('left', leafId, { kind: 'file', id: path, label });
	}

	function findFirstLeaf(node: PaneNode): LeafPane | null {
		if (node.type === 'leaf') return node;
		return findFirstLeaf(node.children[0]) ?? findFirstLeaf(node.children[1]);
	}

	const sidebarWidth = $derived(
		layout?.left.sidebar?.collapsed
			? '28px'
			: `${(layout?.left.sidebar?.widthFraction ?? 0.3) * 100}%`
	);
	const leftWidth = $derived(layout ? `${layout.divider * 100}%` : '25%');
</script>

<div class="tiling-shell" onmouseup={() => { if (dragging) { dragging = false; dragSource = null; } }}>
	{#if restorationBanner && !bannerDismissed}
		<div class="restoration-banner">
			<span>{restorationBanner}</span>
			<button onclick={() => bannerDismissed = true}>×</button>
		</div>
	{/if}

	{#if layout}
		<div class="main-area">
			<!-- Left region -->
			<div class="left-region" style="width: {leftWidth}; flex-shrink: 0;">
				<div class="left-inner">
					<!-- File sidebar -->
					<div class="sidebar-wrap" style="width: {sidebarWidth}; flex-shrink: 0;">
						<FileSidebar
							{agentBase}
							{namespace}
							{project}
							collapsed={layout.left.sidebar?.collapsed ?? false}
							onToggleCollapse={toggleSidebar}
							onOpenFile={openFileInLeft}
						/>
					</div>
					{#if !(layout.left.sidebar?.collapsed)}
						<TilingSplitHandle
							direction="h"
							onDrag={(delta, total) => handleSidebarWidthDrag(delta, total)}
						/>
					{/if}
					<!-- Left pane area -->
					<div class="pane-area" style="flex: 1; min-width: 0; overflow: hidden;">
						<TilingRegion
							node={layout.left.panes}
							nodeId="left"
							regionKind="left"
							{agentBase}
							{namespace}
							{project}
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
								layout.left.panes = updateSplitRatio(layout.left.panes, id, ratio);
								save();
							}}
						/>
					</div>
				</div>
			</div>

			<!-- Main divider -->
			<TilingSplitHandle direction="h" onDrag={(delta) => handleMainDividerDrag(delta)} />

			<!-- Right region -->
			<div class="right-region" style="flex: 1; min-width: 0; overflow: hidden;">
				<TilingRegion
					node={layout.right.panes}
					nodeId="right"
					regionKind="right"
					{agentBase}
					{namespace}
					{project}
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
						layout.right.panes = updateSplitRatio(layout.right.panes, id, ratio);
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
